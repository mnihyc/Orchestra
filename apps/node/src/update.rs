//! Node commands and supervision use the connector core's verified updater.

use anyhow::Context;
use orch_connector_core::automatic::Updates;
use orch_connector_core::update::{Program, ProgramUpdater};
use std::path::{Path, PathBuf};

/// Explicit local consent, available even when update.auto is kept off.
pub async fn update(
    config: &Path,
    data_dir: Option<PathBuf>,
    version: &str,
) -> anyhow::Result<String> {
    let (_, settings) = crate::composition::settings_of(config, data_dir)?;
    let updater = ProgramUpdater::new(settings.data_dir, version, Program::Node)?;
    let Some(prepared) = updater.prepare().await? else {
        return Ok(format!("This node ({version}) is current."));
    };
    updater.install(&prepared)?;
    // The system service and the bundled container both restart the node on failure.
    // An external invocation asks the installed service to restart rather than killing unrelated programs.
    if std::process::Command::new("systemctl")
        .args(["is-active", "--quiet", "orch-node.service"])
        .status()
        .is_ok_and(|status| status.success())
    {
        let status = std::process::Command::new("systemctl")
            .args(["restart", "orch-node.service"])
            .status()
            .context("restarting orch-node.service")?;
        anyhow::ensure!(
            status.success(),
            "The verified build is installed; restart orch-node.service with the service manager."
        );
    }
    Ok(format!(
        "Node {} is installed (was {version}). Its service manager must start the new executable.",
        prepared.offer.version
    ))
}
