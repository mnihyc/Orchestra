//! A local plan removes one connector instance and only its recorded owned directories.

use std::path::{Path, PathBuf};

use anyhow::Context;
use orch_connector_core::enrollment::DataDir;
use orch_connector_core::service::{self, Action, Service};
use orch_connector_core::uninstall::{self, InstanceLock, Item, Plan, Removing};

/// Executes only after the complete local plan has been confirmed once.
pub async fn run(
    data_dir: Option<PathBuf>,
    options: &crate::arguments::Uninstall,
) -> anyhow::Result<()> {
    let root = crate::composition::data_dir_of(data_dir);
    let data = DataDir::inspect(&root)?;
    let name = data.record()?.name;
    let receipt = uninstall::inspect(data.root())?.context(
        "this instance has no installer receipt; run its installer once before uninstalling",
    )?;
    let removing = Removing::begin(receipt, &std::env::current_exe()?)?;
    let receipt = removing.installation().clone();
    let config = data.config(|name| std::env::var(name).ok())?;
    let policy =
        orch_connector_core::policy::PolicyStore::connected(config, Some(data.config_path()))?
            .policy()
            .context("this is not a locally owned connector")?;
    let owned = orch_connector_core::storages::owned_data(data.root())?;
    let mut items = uninstall::installation_items(&removing, options.purge);
    for directory in &owned {
        let path = Path::new(&directory.directory.path);
        let blocker = if path.exists() {
            match orch_connector_core::policy::inspect_environment_directory(&policy, path) {
                Ok(actual) if actual == directory.directory => None,
                _ => Some("the owned directory was replaced or is no longer allowed".into()),
            }
        } else {
            None
        };
        items.push(Item {
            name: path.display().to_string(),
            remove: options.purge,
            bytes: uninstall::size(path),
            blocker,
        });
    }
    if !(Plan { name, items }).confirm(options.yes, options.plan)? {
        return Ok(());
    }
    let cancel = uninstall::Cancellation::listen();
    cancel.check()?;
    let service = Service {
        program: receipt.program.clone(),
        data_dir: receipt.data_dir.clone(),
        suffix: receipt.service_suffix.clone(),
        environment: Vec::new(),
    };
    if receipt.service_installed && !receipt.uninstalled && service::installed(&service) {
        println!("{}", service::act(Action::Stop, &service)?);
    }
    let _instance = InstanceLock::removing(&receipt.data_dir)?;
    cancel.check()?;
    if receipt.service_installed && !receipt.uninstalled {
        println!("{}", service::act(Action::Uninstall, &service)?);
    }
    if options.purge {
        for (index, directory) in owned.iter().enumerate() {
            cancel.check()?;
            orch_connector_core::storages::remove_owned(&policy, directory)?;
            println!(
                "{}/{} removed: {}",
                index + 1,
                owned.len(),
                directory.directory.path
            );
        }
    }
    cancel.check()?;
    let scheduled = uninstall::finish_files(removing, options.purge)?;
    println!(
        "{}",
        if scheduled {
            "The instance is removed. Program removal is scheduled after this command exits."
        } else {
            "The instance is removed."
        }
    );
    Ok(())
}

/// The standalone installer records exact ownership without writing JSON in shell or PowerShell.
pub fn record(
    data_dir: Option<PathBuf>,
    options: &crate::arguments::RecordInstallation,
) -> anyhow::Result<()> {
    let root = crate::composition::data_dir_of(data_dir);
    uninstall::register(
        &std::env::current_exe()?,
        &root,
        Vec::new(),
        options.named,
        options.service,
        true,
    )?;
    Ok(())
}
