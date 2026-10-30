//! Host identity and outbound machine link.
//!
//! A node is a host's one machine to the system. It enrolls like any machine, with a token a person with the
//! right to add a host made (a *host token*), and the system then knows it as a machine of the `node` role whose
//! placement is managed and as the host that runs the units; it keeps its key, its certificate and the
//! authority's in its data directory, renews the certificate before it runs out, and holds one outbound
//! session to the gateway for good. None of that is new here: it is the connector's ([`orch_connector_core`]),
//! and a node is the connector's machinery with the role `node` and no other.
//!
//! This module is what is the node's own: the roles it enrolls with ([`NODE_ROLES`]), the enrollment itself
//! ([`enroll_host`]), and the connector it starts ([`builder`], [`run`]), which is given the node's role and
//! has no policy of its own, since the system made the host's machine and the operator's configuration is the
//! node's only limit.

use std::sync::Arc;

use orch_connector_core::connector::{
    ConnectorBuilder, ConnectorParams, StartError, Supervision, run_until_with_updates,
};
use orch_connector_core::enrollment::{
    DataDir, EnrollError, EnrollInputs, EnrollTransport, Enrolled, enroll,
};
use orch_connector_core::harnesses::AdapterRegistry;
use orch_connector_core::session::{detect_platform, tls_dialer};
use orch_link::content::BlobEndpoint;
use orch_link::reconnect::Dialer;
use orch_link_contract::session::{Platform, Role};
use orch_pki::enrollment::MachineIdentity;
use orch_types::Clock;

use crate::drivers::DriverHost;
use crate::images::BlobDesk;
use crate::role::Prepared;

/// The roles a node enrolls with and plays: `node` and no other. The units it makes play their own.
pub const NODE_ROLES: [Role; 1] = [Role::Node];

/// What a node enrolls with.
#[derive(Debug, Clone)]
pub struct HostEnrollment {
    /// Explicit local consent to hand over the node policy.
    pub delegate_policy: bool,
    /// The system's address: the gateway's, `wss://…`, or the system's own.
    pub address: String,
    /// The host token. It carries the pin of the system's authority, which the node trusts the answer by.
    pub token: String,
    /// What the host is called.
    pub name: String,
}

/// Enrolls the host into a system with its token and keeps the answer in the data directory.
pub async fn enroll_host(
    data: &DataDir,
    host: &HostEnrollment,
    version: &str,
    transport: &dyn EnrollTransport,
    clock: &dyn Clock,
) -> Result<Enrolled, EnrollError> {
    let replacing = data.is_enrolled();
    let inputs = EnrollInputs {
        address: host.address.clone(),
        token: host.token.clone(),
        name: host.name.clone(),
        roles: NODE_ROLES.to_vec(),
        platform: detect_platform(),
        connector_version: version.to_owned(),
        managed: true,
    };
    let bound = host
        .token
        .parse::<orch_link_contract::enrollment::EnrollmentToken>()
        .map_err(|_| EnrollError::BadToken)?
        .choices()
        .map_err(|_| EnrollError::BadToken)?;
    // Older host/bootstrap tokens keep their installed behavior. A current role-choice
    // token requires the exact local flag, and never implicitly hands everything over.
    if bound.is_some() || host.delegate_policy {
        let choices = orch_link_contract::enrollment_choices::EnrollmentChoices::new(
            NODE_ROLES.to_vec(),
            if host.delegate_policy {
                vec!["*".into()]
            } else {
                Vec::new()
            },
            false,
        )
        .map_err(|message| EnrollError::Config(message.into()))?;
        orch_connector_core::enrollment::enroll_with_choices(
            &inputs, data, transport, clock, &choices,
        )
        .await
    } else {
        let enrolled = enroll(&inputs, data, transport, clock).await?;
        if !replacing {
            data.update_config(|config| config.policy.delegated = vec!["*".into()])?;
        }
        Ok(enrolled)
    }
}

/// What a program decides to start a node with.
pub struct Wiring {
    /// The node's version, as the hello says it.
    pub version: String,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
    /// What the node is built for.
    pub platform: Platform,
    /// The files the node keeps.
    pub prepared: Prepared,
    /// The drivers.
    pub drivers: Arc<DriverHost>,
    /// How blobs are fetched for the drivers.
    pub desk: Arc<BlobDesk>,
    /// How content is moved, where it is not by HTTPS to the gateway's endpoint.
    pub blobs: Option<Arc<dyn BlobEndpoint>>,
}

impl Wiring {
    /// A wiring for this machine, with the platform it runs on.
    #[must_use]
    pub fn new(
        version: &str,
        clock: Arc<dyn Clock>,
        prepared: Prepared,
        drivers: Arc<DriverHost>,
        desk: Arc<BlobDesk>,
    ) -> Self {
        Self {
            version: version.to_owned(),
            clock,
            platform: detect_platform(),
            prepared,
            drivers,
            desk,
            blobs: None,
        }
    }
}

/// The trust mode implied by the existing scalar node address setting.
#[must_use]
pub fn endpoint(address: &str) -> orch_link_contract::enrollment::Endpoint {
    use orch_link_contract::enrollment::{Endpoint, Trust};
    let public = reqwest::Url::parse(address)
        .is_ok_and(|url| url.scheme() == "https" || url.path().trim_end_matches('/') == "/link");
    let mut url = address.to_owned();
    if let Ok(mut parsed) = reqwest::Url::parse(address) {
        let _ = parsed.set_scheme("wss");
        url = parsed.to_string();
    }
    Endpoint {
        url,
        trust: if public { Trust::Public } else { Trust::Pinned },
    }
}

/// The connector of a node, ready to start: the identity enrollment left, reached by `dialer` at the
/// endpoints the data directory's configuration lists, playing the role `node`.
pub fn builder(
    wiring: &Wiring,
    data: &DataDir,
    identity: MachineIdentity,
    dialer: Arc<dyn Dialer>,
    var: &(dyn Fn(&str) -> Option<String> + Sync),
) -> Result<ConnectorBuilder, StartError> {
    let config = data.config(var)?;
    let settings = wiring.prepared.settings();
    let local = if config.endpoints.is_empty() && !settings.gateway_url.is_empty() {
        Some(vec![endpoint(&settings.gateway_url)])
    } else {
        None
    };
    let hosted =
        (!settings.managed_endpoint.is_empty()).then(|| vec![endpoint(&settings.managed_endpoint)]);
    let params = ConnectorParams {
        follow_system_endpoints: local.is_none(),
        hosted_endpoints: hosted,
        version: wiring.version.clone(),
        data_dir: data.root().to_owned(),
        endpoints: match local {
            Some(endpoints) => endpoints,
            None => data.endpoints(&config)?,
        },
        config,
        identity,
        roles: NODE_ROLES.to_vec(),
        managed: true,
        dialer,
        clock: Arc::clone(&wiring.clock),
        platform: wiring.platform,
        adapters: AdapterRegistry::new(),
        tools_dir: None,
        blobs: wiring.blobs.clone(),
    };
    Ok(ConnectorBuilder::new(params)
        .installation(orch_connector_core::installation::node(
            data,
            wiring
                .prepared
                .config_file()
                .unwrap_or_else(|| std::path::Path::new("/etc/orch/orch.toml")),
        ))
        .role(
            wiring
                .prepared
                .clone()
                .role(Arc::clone(&wiring.drivers), Arc::clone(&wiring.desk)),
        ))
}

/// Runs the node until `shutdown` resolves: starts the connector from the identity in the data directory,
/// renews the certificate when it is due (starting the connector again with the new one), and stops it.
pub async fn run(
    wiring: &Wiring,
    data: &DataDir,
    transport: &dyn EnrollTransport,
    var: &(dyn Fn(&str) -> Option<String> + Sync),
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), StartError> {
    run_with_updates(wiring, data, transport, var, None, shutdown).await
}

/// The ordinary node supervisor with the physical program's shared verified updater.
pub async fn run_with_updates(
    wiring: &Wiring,
    data: &DataDir,
    transport: &dyn EnrollTransport,
    var: &(dyn Fn(&str) -> Option<String> + Sync),
    updates: Option<&dyn orch_connector_core::automatic::Updates>,
    shutdown: impl std::future::Future<Output = ()>,
) -> Result<(), StartError> {
    let clock = Arc::clone(&wiring.clock);
    let make = |identity: MachineIdentity| -> Result<ConnectorBuilder, StartError> {
        let dialer = Arc::new(tls_dialer(&identity, Arc::clone(&clock))?);
        builder(wiring, data, identity, dialer, var)
    };
    run_until_with_updates(
        data,
        &make,
        transport,
        Arc::clone(&clock),
        orch_connector_core::connector::RunOptions {
            local_endpoints: (!wiring.prepared.settings().gateway_url.is_empty())
                .then(|| vec![endpoint(&wiring.prepared.settings().gateway_url)]),
            supervision: Supervision::default(),
            updates,
        },
        var,
        shutdown,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_node_enrolls_with_the_role_node_and_no_other() {
        assert_eq!(NODE_ROLES, [Role::Node]);
    }

    #[test]
    fn the_file_enrolling_wrote_for_a_node_reads_as_the_roles_it_plays() {
        let dir = tempfile::tempdir().unwrap();
        let data = DataDir::open(dir.path()).unwrap();
        std::fs::write(
            data.config_path(),
            "[connector]\nname = \"host\"\nroles = [\"node\"]\n\n[[endpoints]]\nurl = \"wss://g:9443\"\ntrust = \"pinned\"\n",
        )
        .unwrap();
        let config = data.config(|_| None).unwrap();
        assert_eq!(config.connector.name, "host");
        assert_eq!(
            orch_connector_core::enrollment::roles_of(&config),
            NODE_ROLES
        );
        assert_eq!(config.endpoints[0].url, "wss://g:9443");
    }
}
