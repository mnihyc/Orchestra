//! Hourly recommended-build updates, staged and verified before a fenced idle-only installation.

use crate::connector::{Connector, IDLE_LOOK_EVERY};
use crate::enrollment::DataDir;
use crate::update::{Offer, UpdateError, newer};
use async_trait::async_trait;
use orch_link::machine::LinkStatus;
use orch_link_contract::commands::{
    CommandError, ErrorCode, MachineUpdateNow, MachineUpdateNowResult,
};
use orch_link_contract::events::{Event, MachineUpdateFailed, MachineUpdateRequested};
use orch_link_contract::streams::Stream;
use orch_link_contract::version::RejectCode;
use orch_types::Time;
use std::path::PathBuf;
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

/// A server drain grant, shared by the dispatcher and the physical program's supervisor.
pub struct Requests {
    physical: bool,
    program: crate::update::Program,
    version: String,
    approved: Mutex<Option<String>>,
    supported: AtomicBool,
    changed: tokio::sync::Notify,
}

impl Requests {
    /// A managed node is physical; a managed environment/provider connector is not.
    pub fn new(physical: bool, program: crate::update::Program, version: String) -> Self {
        Self {
            physical,
            program,
            version,
            approved: Mutex::new(None),
            supported: AtomicBool::new(false),
            changed: tokio::sync::Notify::new(),
        }
    }

    /// The composed program has provided its verified install seam.
    pub fn enable(&self) {
        self.supported.store(true, Ordering::Release);
    }

    /// Answers promptly; a busy local turn is drained by the supervisor before it swaps anything.
    pub fn request(
        &self,
        command: &MachineUpdateNow,
        enabled: bool,
    ) -> Result<MachineUpdateNowResult, CommandError> {
        if !self.physical || !self.supported.load(Ordering::Acquire) {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "this program does not self-update; update its environment or provider runtime",
            ));
        }
        if !enabled {
            return Err(CommandError::new(ErrorCode::Unsupported, self.refusal()));
        }
        let newer = newer(&command.recommended, &self.version);
        if newer {
            *self
                .approved
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(command.recommended.clone());
            self.changed.notify_one();
        }
        Ok(MachineUpdateNowResult {
            status: if newer {"scheduled"} else {"current"}.into(),
            running_version: self.version.clone(), recommended_version: command.recommended.clone(),
            detail: if newer {"Update scheduled. The verified build will be installed after the last running turn ends."} else {"This program already runs the recommended version."}.into(),
        })
    }

    fn take(&self) -> Option<String> {
        self.approved
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    fn refusal(&self) -> String {
        format!(
            "update.auto is off on this machine. Its local policy refuses Update now. Run `{}` on the machine to update it locally.",
            self.program.command()
        )
    }
}

/// A recommendation is checked hourly; a required upgrade also wakes on the authenticated refusal.
pub const CHECK_EVERY: Duration = Duration::from_secs(3600);

struct Schedule {
    next: Time,
}

impl Schedule {
    fn new(now: Time) -> Self {
        Self { next: now }
    }
    fn take_due(&mut self, now: Time) -> bool {
        if now < self.next {
            return false;
        }
        self.next = Time::new(now.get() + chrono::Duration::seconds(CHECK_EVERY.as_secs() as i64));
        true
    }
}

/// An already downloaded build with verified size/hash, still separate from the running executable.
#[derive(Debug)]
pub struct Prepared {
    /// The system's validated platform offer.
    pub offer: Offer,
    /// Owned staged download path.
    pub path: PathBuf,
    staging: tempfile::TempDir,
}

impl Prepared {
    /// Retains the owned staging directory until installation or cancellation; concurrent manual/automatic downloads cannot share bytes.
    #[must_use]
    pub fn new(offer: Offer, path: PathBuf, staging: tempfile::TempDir) -> Self {
        Self {
            offer,
            path,
            staging,
        }
    }

    /// The owned staging root, kept alive while the validated path is in use.
    #[must_use]
    pub fn staging_dir(&self) -> &std::path::Path {
        self.staging.path()
    }
}

/// The manual and automatic commands stage into distinct owned directories, checking every download before use.
pub async fn prepare(
    client: &crate::https::HttpsClient,
    base: &url::Url,
    offer: Offer,
    cache: &std::path::Path,
) -> Result<Prepared, UpdateError> {
    std::fs::create_dir_all(cache).map_err(|source| UpdateError::Io {
        path: cache.to_owned(),
        source,
    })?;
    let staging = tempfile::Builder::new()
        .prefix("connector-update-")
        .tempdir_in(cache)
        .map_err(|source| UpdateError::Io {
            path: cache.to_owned(),
            source,
        })?;
    let path = crate::update::download(client, base, &offer, staging.path()).await?;
    Ok(Prepared::new(offer, path, staging))
}

/// The program's transport/install seam; tests can observe the idle fence without replacing a test executable.
#[async_trait]
pub trait Updates: Send + Sync {
    /// Version of the running program.
    fn running_version(&self) -> &str;
    /// Reads the same verified listing and download as the manual update command.
    async fn prepare(&self) -> Result<Option<Prepared>, UpdateError>;
    /// Atomically installs a prepared build, only while the caller holds the idle fence.
    fn install(&self, prepared: &Prepared) -> Result<(), UpdateError>;
}

fn requested(
    managed: bool,
    enabled: bool,
    required: bool,
    offered: Option<&str>,
    running: &str,
) -> bool {
    !managed && (required || (enabled && offered.is_some_and(|offered| newer(offered, running))))
}

fn enabled(connector: &Connector) -> bool {
    // Reload first: a locally changed off value wins even while a download is in progress.
    let _ = connector.services().policy.reload_from_file();
    connector
        .services()
        .policy
        .policy()
        .is_some_and(|policy| policy.update_auto)
}

fn required(status: &LinkStatus) -> bool {
    matches!(
        status,
        LinkStatus::Refused {
            code: RejectCode::UpgradeRequired,
            ..
        }
    )
}

/// Resolves only after a verified build was installed under an idle fence. The caller stops before dropping it.
pub async fn until_installed(
    connector: &Connector,
    data: &DataDir,
    updates: Option<&dyn Updates>,
) -> tokio::sync::OwnedRwLockWriteGuard<()> {
    let Some(updates) = updates else {
        return std::future::pending().await;
    };
    let managed = data.record().map_or(true, |record| record.managed)
        && !connector
            .services()
            .config
            .connector
            .roles
            .iter()
            .any(|role| role == "node");
    if managed {
        return std::future::pending().await;
    }
    connector.update_requests.enable();
    let mut ticker = tokio::time::interval(IDLE_LOOK_EVERY);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut status = connector.link().status();
    let mut schedule = Schedule::new(connector.services().clock.time());
    loop {
        let hourly = tokio::select! {
            _ = ticker.tick() => schedule.take_due(connector.services().clock.time()),
            changed = status.changed() => { if changed.is_err() { return std::future::pending().await; } false },
            () = connector.update_requests.changed.notified() => false,
        };
        let mandatory = required(&status.borrow());
        let versions = connector.link().versions();
        let minimum_permit = match &*status.borrow() {
            LinkStatus::Refused {
                code: RejectCode::UpgradeRequired,
                update_permit: Some(permit),
                ..
            } => Some(permit.recommended.clone()),
            _ => None,
        };
        let approved = connector.update_requests.take().or(minimum_permit);
        if approved.is_none()
            && ((!hourly && !mandatory)
                || !requested(
                    managed,
                    enabled(connector),
                    mandatory,
                    versions
                        .as_ref()
                        .map(|versions| versions.recommended.as_str()),
                    updates.running_version(),
                ))
        {
            continue;
        }
        if approved.is_none() && !mandatory {
            // The server sees turns using any unit, environment, harness or key on this physical
            // machine. Only its drain grant permits an automatic installation.
            if let Some(versions) = versions {
                let _ = connector
                    .link()
                    .publish(
                        &Stream::Machine,
                        &Event::MachineUpdateRequested(MachineUpdateRequested {
                            recommended: versions.recommended,
                        }),
                    )
                    .await;
            }
            continue;
        }
        // A refused node cannot infer that its units are idle. Fail closed until the server can grant it.
        if approved.is_none()
            && connector
                .services()
                .config
                .connector
                .roles
                .iter()
                .any(|role| role == "node")
        {
            continue;
        }
        // Wait for work that was already admitted, and keep honoring a switch turned off meanwhile.
        while connector.turns_running() > 0 {
            if !mandatory && !enabled(connector) {
                break;
            }
            tokio::time::sleep(IDLE_LOOK_EVERY).await;
        }
        if !mandatory && !enabled(connector) {
            failed(connector, connector.update_requests.refusal()).await;
            continue;
        }
        let prepared = match updates.prepare().await {
            Ok(Some(prepared)) => prepared,
            Ok(None) => {
                failed(
                    connector,
                    "The release listing offers no newer build; the running version is kept."
                        .into(),
                )
                .await;
                continue;
            }
            Err(error) => {
                failed(connector, error.to_string()).await;
                continue;
            }
        };
        if !newer(&prepared.offer.version, updates.running_version()) {
            failed(
                connector,
                "The staged offer is not newer than the running version.".into(),
            )
            .await;
            continue;
        }
        if approved
            .as_ref()
            .is_some_and(|version| newer(version, &prepared.offer.version))
        {
            failed(connector,"The staged offer is behind the approved recommendation; the running version is kept.".into()).await;
            continue;
        }
        if let Some(versions) = connector.link().versions()
            && newer(&versions.recommended, &prepared.offer.version)
        {
            tracing::warn!(
                "the download listing is behind the gateway's recommendation; keeping the running build"
            );
            failed(connector,"The download listing is behind the server's recommendation; the running version is kept.".into()).await;
            continue;
        }
        loop {
            if !mandatory && !enabled(connector) {
                failed(connector, connector.update_requests.refusal()).await;
                break;
            }
            let Some(guard) = connector.idle_for_update() else {
                tokio::time::sleep(IDLE_LOOK_EVERY).await;
                continue;
            };
            // Recheck after the fence: a locally disabled automatic installation does not slip through a busy download.
            if !mandatory && !enabled(connector) {
                failed(connector, connector.update_requests.refusal()).await;
                break;
            }
            match updates.install(&prepared) {
                Ok(()) => {
                    tracing::info!(version=%prepared.offer.version,"the idle connector installed its verified update");
                    return guard;
                }
                Err(error) => failed(connector, error.to_string()).await,
            }
            break;
        }
    }
}

async fn failed(connector: &Connector, detail: String) {
    tracing::warn!(%detail, "the verified update kept the running build");
    let _ = connector
        .link()
        .publish(
            &Stream::Machine,
            &Event::MachineUpdateFailed(MachineUpdateFailed { detail }),
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::requested;

    #[test]
    fn a_kept_local_update_auto_off_refuses_both_physical_programs_with_their_command() {
        use crate::policy::PolicyStore;
        use crate::update::Program;
        use orch_link_contract::commands::MachineUpdateNow;
        use orch_link_contract::policy::PolicyKey;
        for program in [Program::Connector, Program::Node] {
            let store =
                PolicyStore::connected(orch_config::ConnectorConfig::default(), None).unwrap();
            store
                .set_here(PolicyKey::UpdateAuto, &serde_json::json!(false))
                .unwrap();
            let requests = super::Requests::new(true, program, "1.0.0".into());
            requests.enable();
            let error = requests
                .request(
                    &MachineUpdateNow {
                        recommended: "1.1.0".into(),
                    },
                    store.policy().unwrap().update_auto,
                )
                .unwrap_err();
            assert!(error.detail.contains("update.auto"));
            assert!(error.detail.contains(program.command()));
            assert!(
                requests.take().is_none(),
                "a refusal never grants an installation"
            );
            store
                .set_here(PolicyKey::UpdateAuto, &serde_json::json!(true))
                .unwrap();
            let accepted = requests
                .request(
                    &MachineUpdateNow {
                        recommended: "1.1.0".into(),
                    },
                    store.policy().unwrap().update_auto,
                )
                .unwrap();
            assert_eq!(accepted.status, "scheduled");
            assert_eq!(requests.take().as_deref(), Some("1.1.0"));
        }
    }

    #[test]
    fn the_hour_is_decided_by_the_injected_clock_without_waiting_for_a_real_hour() {
        use orch_types::Clock;
        let clock = orch_testkit::clock::manual();
        let mut schedule = super::Schedule::new(clock.time());
        assert!(schedule.take_due(clock.time()));
        clock.advance(chrono::Duration::seconds(3599));
        assert!(!schedule.take_due(clock.time()));
        clock.advance(chrono::Duration::seconds(1));
        assert!(schedule.take_due(clock.time()));
        assert!(!schedule.take_due(clock.time()));
        assert_eq!(super::CHECK_EVERY.as_secs(), 3600);
    }

    #[test]
    fn managed_connectors_never_self_update_and_the_owner_can_stop_automatic_recommendations() {
        assert!(!requested(true, true, true, Some("2.0.0"), "1.0.0"));
        assert!(!requested(false, false, false, Some("2.0.0"), "1.0.0"));
        assert!(requested(false, true, false, Some("2.0.0"), "1.0.0"));
        assert!(!requested(false, true, false, Some("1.0.0"), "1.0.0"));
        assert!(requested(false, false, true, None, "1.0.0"));
    }
}
