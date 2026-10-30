//! `update`: the system's newer build in this program's place, and the service started again on it.
//!
//! The listing and the build come from the system's own address, as the installers fetch them: it is believed by
//! the roots a browser believes, or else by the system's own authority, which a system whose address shows a
//! certificate of that authority needs. Nothing changes when the system offers no newer build.

use std::path::Path;

use anyhow::Context;
use orch_build_info::BuildInfo;
use orch_connector_core::https::HttpsClient;
use orch_connector_core::service::{self, Action, Service};
use orch_connector_core::update::{Offer, UpdateError, install_verified, newer};

/// Updates the connector whose data directory is `dir` (`named` on the command line or the platform's own), which
/// runs `build`; says what came of it.
pub async fn update(dir: &Path, named: bool, build: BuildInfo) -> anyhow::Result<String> {
    let (client, base, offer) = inspect(dir).await?;
    if !newer(&offer.version, build.version) {
        return Ok(format!(
            "This connector ({}) is current: the system offers {}.",
            build.version, offer.version
        ));
    }
    let staged =
        orch_connector_core::automatic::prepare(&client, &base, offer.clone(), &dir.join("cache"))
            .await
            .context("downloading the new build")?;
    let program = std::env::current_exe().context("finding this program's file")?;
    install_verified(&program, &staged.path, &staged.offer)
        .context("putting the new build in this program's place")?;
    let mut said = vec![format!(
        "The connector is now {} (it was {}), at {}.",
        offer.version,
        build.version,
        program.display()
    )];
    let service = Service::new(program, dir.to_owned(), named);
    if service::installed(&service) {
        // Stopped and started again, so that the service runs the new build.
        let _ = service::act(Action::Stop, &service);
        service::act(Action::Start, &service).context("starting the service again")?;
        said.push("Its service is started again on the new build.".to_owned());
    } else {
        said.push(
            "No service runs it here: a connector started by hand runs the old build until it is started again."
                .to_owned(),
        );
    }
    Ok(said.join("\n"))
}

// The one transport/trust selection for both entry points.
async fn inspect(dir: &Path) -> anyhow::Result<(HttpsClient, url::Url, Offer)> {
    orch_connector_core::update::inspect(dir, orch_connector_core::update::Program::Connector)
        .await
        .context("reading the system's release listing")
}

/// The running program's verified updater; the core decides its hour, policy and idle fence.
pub struct Automatic {
    dir: std::path::PathBuf,
    program: std::path::PathBuf,
    build: BuildInfo,
}

impl Automatic {
    /// Captures the executable's real path before an atomic replacement can mark the old inode deleted.
    pub fn new(dir: std::path::PathBuf, build: BuildInfo) -> anyhow::Result<Self> {
        Ok(Self {
            dir,
            program: std::env::current_exe().context("finding the connector program")?,
            build,
        })
    }

    /// The installed per-user manager restarts failures. A hand-started connector replaces itself on Unix.
    pub fn restart(&self) -> anyhow::Result<()> {
        let default =
            orch_connector_core::enrollment::default_data_dir(|name| std::env::var(name).ok());
        let named = Service::new(self.program.clone(), self.dir.clone(), true);
        let plain = Service::new(self.program.clone(), self.dir.clone(), false);
        if service::installed(&named) || (self.dir == default && service::installed(&plain)) {
            anyhow::bail!(
                "The verified update is installed; leaving so this connector's per-user service manager restarts it."
            );
        }
        let mut command = std::process::Command::new(&self.program);
        command.args(std::env::args_os().skip(1));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            Err(command.exec()).context("starting the updated connector")
        }
        #[cfg(not(unix))]
        {
            command.spawn().context("starting the updated connector")?;
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl orch_connector_core::automatic::Updates for Automatic {
    fn running_version(&self) -> &str {
        self.build.version
    }
    async fn prepare(
        &self,
    ) -> Result<Option<orch_connector_core::automatic::Prepared>, UpdateError> {
        let (client, base, offer) = inspect(&self.dir)
            .await
            .map_err(|error| UpdateError::Listing(format!("{error:#}")))?;
        if !newer(&offer.version, self.build.version) {
            return Ok(None);
        }
        orch_connector_core::automatic::prepare(&client, &base, offer, &self.dir.join("cache"))
            .await
            .map(Some)
    }
    fn install(
        &self,
        prepared: &orch_connector_core::automatic::Prepared,
    ) -> Result<(), UpdateError> {
        install_verified(&self.program, &prepared.path, &prepared.offer)
    }
}
