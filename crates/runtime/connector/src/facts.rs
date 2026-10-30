//! What the machine reports of itself: the host, its roles' contributions, its local policy and its link.
//!
//! A machine's facts are the first thing the system learns of it, and they come again whenever they
//! change. The host's part is read from the host ([`Host::detect`]); each role adds its own (the
//! adapters the agent role can drive, the harnesses found here); the local policy and the figures of the
//! link are put in beside them by [`machine_facts`].

use crate::harnesses::Hosting;
use orch_agent_contract::adapter::{AgentStart, EventSink};
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::loopback::Loopback;
use orch_link_contract::events::ProviderFound;
use orch_link_contract::policy::LocalPolicy;
use orch_link_contract::session::{
    MachineFacts, MachineFactsAdapter, MachineFactsHarness, MachineFactsPlugin, MachineFactsRates,
};
use std::time::Duration;
use sysinfo::{MemoryRefreshKind, RefreshKind, System};

/// A local discovery has the same bound as a machine probe; it never opens a session or calls a model.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(30);

/// Local discovery follows real reconnects, while the recovery of active sessions remains separate.
pub(crate) async fn on_connections(
    link: orch_link::machine::MachineLink,
    roles: Vec<std::sync::Arc<dyn crate::roles::RoleHandler>>,
    policy: std::sync::Arc<crate::policy::PolicyStore>,
    discovery: std::sync::Arc<crate::local_harnesses::Discovery>,
    facts: std::sync::Arc<crate::dispatch::FactsNow>,
    stop: tokio_util::sync::CancellationToken,
) {
    let mut status = link.status();
    let mut previous = None;
    loop {
        let session = match status.borrow_and_update().clone() {
            orch_link::machine::LinkStatus::Connected { session_id, .. } => Some(session_id),
            _ => None,
        };
        if session != previous {
            previous = session;
            if let Some(session) = session {
                discovery.refresh(&policy).await;
                if let Err(error) = link
                    .publish(
                        &orch_link_contract::streams::Stream::Machine,
                        &orch_link_contract::events::Event::MachineFacts(facts()),
                    )
                    .await
                {
                    tracing::warn!(%error, "local programs could not be reported");
                }
                for role in &roles {
                    role.connected(session).await;
                }
            } else {
                for role in &roles {
                    role.disconnected().await;
                }
            }
        }
        tokio::select! {
            () = stop.cancelled() => return,
            changed = status.changed() => if changed.is_err() { return; },
        }
    }
}

/// Reads existing sign-ins only from harnesses the connected machine's owner already allowed.
pub(crate) async fn local_sign_ins(hosting: &Hosting) -> Vec<ProviderFound> {
    let services = hosting.services();
    let Some(policy) = services.policy.policy() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for harness in &policy.agent_harnesses {
        let Some(registration) = hosting.registry().get(&harness.kind) else {
            continue;
        };
        let mut adapter = (registration.factory)(services.clock.clone());
        let grant = services.loopback.grant();
        let (sink, _events) = EventSink::channel();
        let probe = async {
            let started = adapter
                .start(
                    AgentStart {
                        // Discovery owns no provider or profile: these identifiers never leave the local probe.
                        provider_id: orch_types::Id::nil(),
                        agent_id: orch_types::Id::nil(),
                        harness_kind: harness.kind.clone(),
                        profile: AgentProfile::default(),
                        home: None,
                        provider_config: serde_json::json!({}),
                        loopback: Loopback::new(services.loopback.port(), grant.token.clone()),
                        binary: Some(harness.path.clone()),
                    },
                    sink,
                )
                .await
                .ok()?;
            started.descriptor.native_provider.as_ref()?;
            let facts = adapter.provider_facts().await.ok()?;
            facts.credential.as_ref()?;
            Some(ProviderFound {
                harness_kind: harness.kind.clone(),
                identity: facts.identity,
            })
        };
        if let Ok(Some(login)) = tokio::time::timeout(DISCOVERY_TIMEOUT, probe).await {
            found.push(login);
        }
        let _ = tokio::time::timeout(DISCOVERY_TIMEOUT, adapter.stop()).await;
        services.loopback.revoke(grant.id);
    }
    found
}

/// What the roles add to the facts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FactsDraft {
    /// The actual account's independently proved native resource controllers.
    pub process_controls: orch_driver_contract::process_limits::ProcessControls,
    /// Local-only machine isolation declaration, read from the same configuration file.
    pub dedicated: bool,
    /// Whether the hosting program actually fetches named builds.
    pub build_fetch: Option<orch_driver_contract::descriptor::OperationSupport>,
    /// Named storages supplied by the roles that can make owned data.
    pub storages: Vec<orch_link_contract::events::NodeStorage>,
    /// Offered by an actual local environment role, not inferred from placement.
    pub environment_data_storage: Option<orch_driver_contract::descriptor::EnvironmentDataStorage>,
    /// Actual local harness-home support, contributed independently of environment registration.
    pub provider_home_storage: Option<orch_driver_contract::descriptor::EnvironmentDataStorage>,
    /// The harness kinds the connector can drive.
    pub adapters: Vec<MachineFactsAdapter>,
    /// The harnesses found on the machine.
    pub harnesses: Vec<MachineFactsHarness>,
    /// The implementations from outside registered on it.
    pub plugins: Vec<MachineFactsPlugin>,
}

/// What the host is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// Its name.
    pub hostname: String,
    /// The version of its operating system.
    pub os_version: String,
    /// How many processors it has.
    pub cpus: u32,
    /// How much memory it has.
    pub memory_bytes: i64,
}

impl Host {
    /// Reads the host.
    #[must_use]
    pub fn detect() -> Self {
        let system = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::nothing().with_ram()),
        );
        Self {
            hostname: System::host_name().unwrap_or_else(|| "unknown".to_owned()),
            os_version: System::long_os_version()
                .or_else(System::os_version)
                .unwrap_or_else(|| "unknown".to_owned()),
            cpus: std::thread::available_parallelism()
                .map_or(1, |n| u32::try_from(n.get()).unwrap_or(u32::MAX)),
            memory_bytes: i64::try_from(system.total_memory()).unwrap_or(i64::MAX),
        }
    }
}

/// The facts of a machine.
#[must_use]
pub fn machine_facts(
    host: &Host,
    draft: FactsDraft,
    policy: Option<LocalPolicy>,
    rates: MachineFactsRates,
    clock_offset_ms: i64,
) -> MachineFacts {
    MachineFacts {
        role_management: None,
        programs: None,
        installation: None,
        provider_runtime_descriptor: draft.provider_home_storage.map(|storage| {
            let mut descriptor =
                orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
                    env!("CARGO_PKG_VERSION"),
                    storage,
                )
                .dedicated_machine(draft.dedicated);
            descriptor.resource_limits = draft.process_controls.support();
            descriptor.live_changes.resources = draft.process_controls.any();
            descriptor
        }),
        build_fetch: draft.build_fetch,
        storages: draft.storages,
        environment_descriptor: draft.environment_data_storage.clone().map(|storage| {
            let mut descriptor =
                orch_driver_contract::descriptor::DriverDescriptor::registered_directories(
                    env!("CARGO_PKG_VERSION"),
                    storage,
                )
                .dedicated_machine(draft.dedicated);
            descriptor.resource_limits = draft.process_controls.support();
            descriptor.live_changes.resources = draft.process_controls.any();
            descriptor
        }),
        hostname: host.hostname.clone(),
        os_version: host.os_version.clone(),
        cpus: host.cpus,
        memory_bytes: host.memory_bytes,
        // Local configuration can name a program before it is installed. Report only files
        // actually found; an adapter's successful descriptor probe establishes its capabilities.
        harnesses: draft
            .harnesses
            .into_iter()
            .filter(|harness| std::path::Path::new(&harness.path).is_file())
            .collect(),
        discovered_harnesses: None,
        searched_harness_kinds: None,
        adapters: draft.adapters,
        plugins: draft.plugins,
        policy,
        rates,
        clock_offset_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_is_read_and_has_at_least_what_every_machine_has() {
        let host = Host::detect();
        assert!(!host.hostname.is_empty());
        assert!(host.cpus >= 1);
        assert!(host.memory_bytes > 0);
    }

    #[test]
    fn a_machine_the_system_made_reports_no_policy_of_its_own() {
        let facts = machine_facts(
            &Host::detect(),
            FactsDraft::default(),
            None,
            MachineFactsRates {
                up_bps: 0,
                down_bps: 0,
                round_trip_ms: 0,
            },
            0,
        );
        assert!(facts.policy.is_none());
    }

    #[test]
    fn naming_a_missing_program_does_not_report_an_installed_harness() {
        let existing = std::env::current_exe().unwrap();
        let missing = existing.join("missing-harness");
        let draft = FactsDraft {
            harnesses: vec![
                MachineFactsHarness {
                    kind: "existing".to_owned(),
                    path: existing.display().to_string(),
                    version: "unknown".to_owned(),
                    daemon: false,
                },
                MachineFactsHarness {
                    kind: "missing".to_owned(),
                    path: missing.display().to_string(),
                    version: "unknown".to_owned(),
                    daemon: false,
                },
            ],
            ..Default::default()
        };
        let facts = machine_facts(
            &Host::detect(),
            draft,
            Some(LocalPolicy::default()),
            MachineFactsRates {
                up_bps: 0,
                down_bps: 0,
                round_trip_ms: 0,
            },
            0,
        );
        assert_eq!(facts.harnesses.len(), 1);
        assert_eq!(facts.harnesses[0].kind, "existing");
    }
}
