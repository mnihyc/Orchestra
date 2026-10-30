//! Machine link connection and reconnect, and the round trip it measures.
//!
//! The link keeps a connector's session to the gateway for good: it tries the endpoints in the order
//! enrollment gave (the one that worked last first), waits one second after a failure and twice as long
//! after each further one up to half a minute with a fifth of jitter, and begins again, for ever
//! ([`orch_link::machine::MachineLink`]). It says hello with what the machine is and has; it measures the
//! round trip from its pings and the rate each way from what it sends and receives.
//!
//! This module is what a connector adds to that: the TLS configurations of the two ways a gateway is
//! trusted, built from the identity enrollment left ([`tls_dialer`]); the link's configuration from the
//! connector's ([`machine_config`]); the platform it is built for ([`detect_platform`]); and a watch on
//! the link's standing that says in the log what a refusal means and what is to be done about it
//! ([`watch`]).

use std::sync::Arc;
use std::time::Duration;

use orch_config::ConnectorConfig;
use orch_link::machine::{LinkStatus, MachineConfig, MachineLink};
use orch_link::spool::SpoolOptions;
use orch_link::tunnels::channel::TunnelOptions;
use orch_link::websocket::TlsDialer;
use orch_link_contract::enrollment::Endpoint;
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_link_contract::version::{RejectCode, Remedy};
use orch_pki::PkiError;
use orch_pki::certificates::ServiceName;
use orch_pki::enrollment::MachineIdentity;
use orch_pki::verification::{ServerTrust, machine_client_config};
use orch_types::Clock;

/// The platform this build runs on.
#[must_use]
pub fn detect_platform() -> Platform {
    Platform {
        os: match std::env::consts::OS {
            "macos" => Os::Macos,
            "windows" => Os::Windows,
            _ => Os::Linux,
        },
        arch: match std::env::consts::ARCH {
            "aarch64" => Arch::Aarch64,
            _ => Arch::X86_64,
        },
    }
}

/// A dialer using the common service-identity or device-trust decision at both entrances: the direct
/// door sees this machine's certificate, while the web edge receives its proof inside the session.
pub fn tls_dialer(
    identity: &MachineIdentity,
    clock: Arc<dyn Clock>,
) -> Result<TlsDialer, PkiError> {
    let tls = identity.tls()?;
    let pinned = machine_client_config(
        Some(ServerTrust::Authority(Box::new(identity.authority.clone()))),
        ServiceName::Gateway,
        Some(&tls),
        Arc::clone(&clock),
    )?;
    let public = machine_client_config(
        Some(ServerTrust::Authority(Box::new(identity.authority.clone()))),
        ServiceName::Frontend,
        None,
        clock,
    )?;
    // Through the system's proxy, where it names one: a restricted unit reaches the gateway only through its
    // host's proxy, which its driver names.
    Ok(TlsDialer::new(Some(pinned), Some(public))
        .with_proxy(Some(orch_link::outward::OutwardProxy::system())))
}

/// What the link is started with, from what the connector has.
pub struct LinkSettings<'a> {
    /// The connector's version, as the hello says it.
    pub version: &'a str,
    /// What this machine is built for.
    pub platform: Platform,
    /// The roles it plays.
    pub roles: Vec<Role>,
    /// Who it is.
    pub identity: &'a MachineIdentity,
    /// Where the gateway is, in the order to try.
    pub endpoints: Vec<Endpoint>,
    /// Where the spool and the journal are kept.
    pub data_dir: &'a std::path::Path,
    /// Its configuration.
    pub config: &'a ConnectorConfig,
    /// Whether local configuration permits refreshing from the system list.
    pub follow_system_endpoints: bool,
    /// A node's explicit addresses for hosted units.
    pub hosted_endpoints: Option<Vec<Endpoint>>,
    /// Whether a person connected it.
    pub connected: bool,
}

/// The link's configuration.
pub fn machine_config(settings: LinkSettings<'_>) -> Result<MachineConfig, PkiError> {
    Ok(MachineConfig {
        hosted_endpoints: settings.hosted_endpoints,
        follow_system_endpoints: settings.follow_system_endpoints
            && settings.config.endpoints.is_empty(),
        machine_id: settings.identity.machine_id,
        connector_version: settings.version.to_owned(),
        platform: settings.platform,
        roles: settings.roles,
        proof: Some((
            settings.identity.key.duplicate()?,
            settings.identity.certificate.clone(),
        )),
        endpoints: settings.endpoints,
        data_dir: settings.data_dir.to_owned(),
        spool: SpoolOptions::from(&settings.config.spool),
        journal_keep: Duration::from_secs(settings.config.journal.keep_hours.saturating_mul(3_600)),
        connected: settings.connected,
        tunnels: TunnelOptions::default(),
        tunnel_gauge: None,
    })
}

/// What a refusal of the session means for a connector, in a sentence for its log.
#[must_use]
pub fn refusal_advice(code: RejectCode) -> &'static str {
    match code.remedy() {
        Some(Remedy::Renew) => {
            "the certificate has run out: it is renewed, and the session tried again"
        }
        Some(Remedy::Update) => {
            "this connector is older than the system accepts: it has to be updated"
        }
        Some(Remedy::Wait) => {
            "the system cannot answer just now: the connector waits and tries again"
        }
        None => {
            "only a person can lift this: check the machine in the system, and the clock of this one"
        }
    }
}

/// One line for the log about how the link stands.
#[must_use]
pub fn describe(status: &LinkStatus) -> String {
    match status {
        LinkStatus::Starting => "the link is starting".to_owned(),
        LinkStatus::Connecting { failures } => {
            format!("connecting to the gateway ({failures} attempts have failed before)")
        }
        LinkStatus::Connected { endpoint, .. } => format!("a session stands, through {endpoint}"),
        LinkStatus::Waiting { retry_in, why, .. } => {
            format!(
                "no session; trying again in {} s ({why})",
                retry_in.as_secs()
            )
        }
        LinkStatus::Refused { code, detail, .. } => {
            format!(
                "the gateway refused the session ({code:?}, {detail}): {}",
                refusal_advice(*code)
            )
        }
        LinkStatus::Stopped => "the link is stopped".to_owned(),
    }
}

/// Writes every change of the link's standing to the log, until the link stops.
pub async fn watch(link: MachineLink) {
    let mut status = link.status();
    loop {
        let line = describe(&status.borrow_and_update());
        match &*status.borrow() {
            LinkStatus::Refused { .. } => tracing::error!("{line}"),
            LinkStatus::Waiting { .. } => tracing::warn!("{line}"),
            _ => tracing::info!("{line}"),
        }
        if status.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn what_the_link_is_started_with_follows_the_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = ConnectorConfig::default();
        config.spool.max_age_days = 3;
        config.journal.keep_hours = 2;
        let identity = {
            let clock: Arc<dyn Clock> = Arc::new(orch_types::SystemClock);
            let authority =
                orch_pki::ca::CertificateAuthority::create("Test", 3650, clock.as_ref()).unwrap();
            let (key, certificate) = authority
                .issue_with_new_key(
                    &orch_pki::ca::Holder::Machine(new_id()),
                    365,
                    clock.as_ref(),
                )
                .unwrap();
            MachineIdentity {
                machine_id: certificate.machine_id().unwrap(),
                key,
                certificate,
                authority: authority.certificate().clone(),
            }
        };
        let settings = LinkSettings {
            follow_system_endpoints: true,
            hosted_endpoints: None,
            version: "0.1.0",
            platform: detect_platform(),
            roles: vec![Role::Agent],
            identity: &identity,
            endpoints: vec![],
            data_dir: dir.path(),
            config: &config,
            connected: true,
        };
        let built = machine_config(settings).unwrap();
        assert_eq!(built.machine_id, identity.machine_id);
        assert_eq!(built.spool.max_age, Duration::from_secs(3 * 86_400));
        assert_eq!(built.journal_keep, Duration::from_secs(2 * 3_600));
        assert!(built.connected);
        assert!(
            built.proof.is_some(),
            "a machine that meets a public edge can prove itself"
        );
    }

    #[test]
    fn every_refusal_says_what_is_to_be_done_about_it() {
        for code in [
            RejectCode::UnknownMachine,
            RejectCode::Revoked,
            RejectCode::BadProof,
            RejectCode::ClockSkew,
            RejectCode::CertificateExpired,
            RejectCode::ProtocolUnsupported,
            RejectCode::UpgradeRequired,
            RejectCode::Busy,
        ] {
            assert!(!refusal_advice(code).is_empty());
        }
        assert!(refusal_advice(RejectCode::CertificateExpired).contains("renewed"));
        assert!(refusal_advice(RejectCode::UpgradeRequired).contains("updated"));
    }

    #[test]
    fn the_standing_of_the_link_reads_as_a_line() {
        assert!(describe(&LinkStatus::Starting).contains("starting"));
        let waiting = LinkStatus::Waiting {
            failures: 2,
            retry_in: Duration::from_secs(4),
            why: "refused".into(),
        };
        assert!(describe(&waiting).contains("4 s"));
    }
}
