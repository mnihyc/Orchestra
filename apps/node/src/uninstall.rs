//! A local uninstall plan uses the same driver-neutral inventory as remote cleanup.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use orch_connector_core::enrollment::DataDir;
use orch_connector_core::uninstall::{self, InstanceLock, Item, Plan, Removing};
use orch_node_core::drivers::{DriverHost, HostDriver, Registrations};
use orch_node_core::images::BlobDesk;
use orch_node_core::ledger::Ledger;
use orch_node_core::resources;
use orch_node_core::settings::NodeSettings;
use orch_node_core::tools::ToolCache;

fn settings(config_path: &Path, data_dir: Option<PathBuf>) -> anyhow::Result<NodeSettings> {
    // Preview never opens the policy store or changes the node's configuration.
    let config: orch_config::ServerConfig = orch_config::load(config_path)?;
    let mut settings = NodeSettings::from_config(&config);
    if let Some(data_dir) = data_dir {
        settings.data_dir = data_dir;
    }
    settings.config_file = Some(config_path.to_owned());
    Ok(settings)
}

fn registrations(settings: &NodeSettings) -> Registrations {
    let mut registrations = Registrations::new().register(
        crate::docker::NAME,
        Arc::new(|context| {
            Box::pin(async move {
                let config = crate::docker::config_of(&context.settings)?;
                let driver =
                    orch_driver_docker::DockerDriver::connect_for_inventory(config).await?;
                Ok(HostDriver::new(Arc::new(driver)))
            })
        }),
    );
    for driver in settings
        .drivers
        .iter()
        .chain(&settings.external_drivers)
        .filter(|name| name.as_str() != crate::docker::NAME)
    {
        registrations = registrations.not_built(
            driver,
            "this driver's read-only local inventory is unavailable",
        );
    }
    registrations
}

fn manager(arguments: &[&str]) -> anyhow::Result<()> {
    let status = std::process::Command::new("systemctl")
        .args(arguments)
        .status()
        .context("asking the node's service manager")?;
    anyhow::ensure!(
        status.success(),
        "the node's service manager refused {}",
        arguments.join(" ")
    );
    Ok(())
}

/// Prints the complete plan before stopping or removing any runtime object or installation file.
pub async fn run(
    config_path: &Path,
    data_dir: Option<PathBuf>,
    options: &crate::arguments::Uninstall,
) -> anyhow::Result<()> {
    let mut settings = settings(config_path, data_dir)?;
    let data = DataDir::inspect(&settings.data_dir)?;
    settings.data_dir = data.root().to_owned();
    let name = data.record()?.name;
    let removing = if options.runtime_only {
        None
    } else {
        let receipt = uninstall::inspect(data.root())?.context(
            "this node has no installer receipt; run its installer once before uninstalling",
        )?;
        Some(Removing::begin(receipt, &std::env::current_exe()?)?)
    };
    let ledger = Ledger::open(data.root())?;
    let tools = ToolCache::inspect(&settings)?;
    let drivers = DriverHost::load(
        &settings,
        registrations(&settings),
        Arc::new(BlobDesk::new()),
    )
    .await;
    let inventory = resources::snapshot(&drivers, &tools, &ledger, &orch_types::SystemClock).await;
    let mut items = removing.as_ref().map_or_else(Vec::new, |removing| {
        uninstall::installation_items(removing, options.purge)
    });
    if let Some(removing) = &removing {
        let configuration = std::fs::canonicalize(config_path)?;
        if !removing
            .installation()
            .configuration
            .contains(&configuration)
        {
            if options.purge && configuration.starts_with(&removing.installation().data_dir) {
                let root = removing.installation().data_dir.display().to_string();
                if let Some(item) = items
                    .iter_mut()
                    .find(|item| item.name == root && item.remove)
                {
                    item.blocker=Some("move the supplied configuration outside the data directory before purging it".into());
                }
            }
            items.push(Item {
                name: configuration.display().to_string(),
                remove: false,
                bytes: uninstall::size(&configuration),
                blocker: Some("supplied configuration is kept".into()),
            });
        }
    }
    items.extend(inventory.resources.iter().map(|resource| {
        Item {
            name: format!(
                "{:?}: {}{}",
                resource.resource.kind,
                resource.name,
                if options.purge
                    && resource.busy
                    && resource.resource.kind == orch_driver_contract::resources::ResourceKind::Unit
                {
                    " (stop first)"
                } else {
                    ""
                }
            ),
            remove: options.purge,
            bytes: resource
                .size_bytes
                .and_then(|bytes| u64::try_from(bytes).ok()),
            blocker: (options.purge
                && resource.busy
                && resource.resource.kind != orch_driver_contract::resources::ResourceKind::Unit)
                .then(|| "another object still uses this object".into()),
        }
    }));
    items.extend(inventory.unavailable.iter().map(|unavailable| Item {
        name: unavailable.driver.clone(),
        remove: options.purge,
        bytes: None,
        blocker: Some(unavailable.reason.clone()),
    }));
    if !(Plan { name, items }).confirm(options.yes, options.plan)? {
        return Ok(());
    }
    let cancel = uninstall::Cancellation::listen();
    cancel.check()?;
    let service = removing.as_ref().is_some_and(|removing| {
        let receipt = removing.installation();
        receipt.service_installed && !receipt.uninstalled
    });
    if service {
        manager(&["stop", "orch-node.service"])?;
        println!("The node service is stopped.");
    }
    let _instance = InstanceLock::removing(&settings.data_dir)?;
    if options.purge {
        let mut completed = 0;
        let total = inventory.resources.len();
        let results = resources::purge_controlled(
            &drivers,
            &tools,
            &ledger,
            &orch_types::SystemClock,
            &inventory.resources,
            cancel.token(),
            |resource, result| {
                completed += 1;
                match result {
                    Ok(()) => println!(
                        "{completed}/{total} removed: {:?} {}",
                        resource.kind, resource.id
                    ),
                    Err(error) => println!(
                        "{completed}/{total} kept: {:?} {} ({})",
                        resource.kind, resource.id, error.detail
                    ),
                }
            },
        )
        .await;
        let failures = results.iter().filter(|(_, result)| result.is_err()).count();
        anyhow::ensure!(
            failures == 0,
            "{failures} objects remain; the node's files and ownership records are kept for a retry"
        );
        let remaining =
            resources::snapshot(&drivers, &tools, &ledger, &orch_types::SystemClock).await;
        anyhow::ensure!(
            remaining.resources.is_empty() && remaining.unavailable.is_empty(),
            "the runtime inventory changed; the node's files are kept for a new plan"
        );
    }
    cancel.check()?;
    if let Some(removing) = removing {
        if service {
            manager(&["disable", "orch-node.service"])?;
            let unit = Path::new("/etc/systemd/system/orch-node.service");
            if unit.exists() {
                std::fs::remove_file(unit)?;
            }
            manager(&["daemon-reload"])?;
        }
        let scheduled = uninstall::finish_files(removing, options.purge)?;
        println!(
            "{}",
            if scheduled {
                "The node is removed. Program removal is scheduled after this command exits."
            } else {
                "The node is removed."
            }
        );
    } else {
        println!("This node's managed runtime objects are removed.");
    }
    Ok(())
}

/// Records only configuration the installer wrote; a supplied configuration is preserved.
pub fn record(
    config_path: &Path,
    data_dir: Option<PathBuf>,
    options: &crate::arguments::RecordInstallation,
) -> anyhow::Result<()> {
    let settings = settings(config_path, data_dir)?;
    let configuration = if options.configuration {
        vec![std::path::absolute(config_path)?]
    } else {
        Vec::new()
    };
    uninstall::register(
        &std::env::current_exe()?,
        &settings.data_dir,
        configuration,
        false,
        options.service,
        true,
    )?;
    Ok(())
}
