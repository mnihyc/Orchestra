//! Local uninstall preserves other instances and refuses ownership or quiescence it cannot prove.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::path::Path;

use orch_connector_core::uninstall::{self, InstanceLock, Removing};

fn install(
    program: &Path,
    data: &Path,
    configuration: Vec<std::path::PathBuf>,
) -> uninstall::Installation {
    std::fs::create_dir_all(data).unwrap();
    uninstall::register(program, data, configuration, true, false, true).unwrap()
}

#[cfg(unix)]
#[test]
fn a_configuration_alias_never_redirects_uninstall_into_an_existing_file() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("orch-connector");
    std::fs::write(&program, b"program").unwrap();
    let owned = root.path().join("owned");
    let foreign = root.path().join("foreign");
    std::fs::create_dir(&owned).unwrap();
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(owned.join("configuration"), b"installed").unwrap();
    std::fs::write(foreign.join("configuration"), b"borrowed").unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&owned, &alias).unwrap();
    let data = root.path().join("data");
    let receipt = install(&program, &data, vec![alias.join("configuration")]);
    std::fs::remove_file(&alias).unwrap();
    std::os::unix::fs::symlink(&foreign, &alias).unwrap();
    uninstall::finish_files(Removing::begin(receipt, &program).unwrap(), false).unwrap();
    assert!(!owned.join("configuration").exists());
    assert_eq!(
        std::fs::read(foreign.join("configuration")).unwrap(),
        b"borrowed"
    );
}

#[cfg(unix)]
#[test]
fn replacing_an_installed_configuration_parent_blocks_the_plan_before_file_removal() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("orch-connector");
    std::fs::write(&program, b"program").unwrap();
    let parent = root.path().join("configuration");
    let foreign = root.path().join("foreign");
    std::fs::create_dir(&parent).unwrap();
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(parent.join("settings"), b"installed").unwrap();
    std::fs::write(foreign.join("settings"), b"borrowed").unwrap();
    let data = root.path().join("data");
    let receipt = install(&program, &data, vec![parent.join("settings")]);
    std::fs::rename(&parent, root.path().join("old-configuration")).unwrap();
    std::os::unix::fs::symlink(&foreign, &parent).unwrap();
    let removing = Removing::begin(receipt, &program).unwrap();
    assert!(
        uninstall::installation_items(&removing, true)
            .iter()
            .any(|item| item.remove && item.blocker.is_some())
    );
    assert!(uninstall::finish_files(removing, true).is_err());
    assert_eq!(
        std::fs::read(foreign.join("settings")).unwrap(),
        b"borrowed"
    );
    assert!(program.is_file());
    assert!(data.is_dir());
}

#[test]
fn only_the_last_instance_removes_the_program_and_retained_data_keeps_its_receipt() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("orch-connector");
    std::fs::write(&program, b"program").unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let supplied = root.path().join("supplied.toml");
    let written = root.path().join("installer.toml");
    std::fs::write(&supplied, b"borrowed").unwrap();
    std::fs::write(&written, b"owned").unwrap();
    let first = install(&program, &a, vec![written.clone()]);
    let second = install(&program, &b, Vec::new());
    let first = Removing::begin(first, &program).unwrap();
    assert_eq!(first.other_instances(), 1);
    assert!(!first.may_remove_program());
    assert!(!uninstall::finish_files(first, false).unwrap());
    assert!(program.is_file());
    assert!(a.is_dir());
    assert!(!written.exists());
    assert_eq!(std::fs::read(&supplied).unwrap(), b"borrowed");
    assert!(uninstall::inspect(&a).unwrap().unwrap().uninstalled);
    let second = Removing::begin(second, &program).unwrap();
    assert!(second.may_remove_program());
    uninstall::finish_files(second, true).unwrap();
    assert!(!b.exists());
    #[cfg(not(windows))]
    assert!(!program.exists());
    assert!(a.join(uninstall::RECEIPT).is_file());
}

#[test]
fn reinstalling_preserves_only_recorded_configuration_and_never_adopts_a_different_program() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("orch-node");
    std::fs::write(&program, b"program").unwrap();
    let data = root.path().join("data");
    let config = root.path().join("node.toml");
    std::fs::write(&config, b"config").unwrap();
    install(&program, &data, vec![config.clone()]);
    let receipt = uninstall::register(&program, &data, Vec::new(), true, false, true).unwrap();
    assert_eq!(
        receipt.configuration,
        vec![std::fs::canonicalize(config).unwrap()]
    );
    let other = root.path().join("other-program");
    std::fs::write(&other, b"another").unwrap();
    assert!(uninstall::register(&other, &data, Vec::new(), true, false, true).is_err());
    assert_eq!(uninstall::inspect(&data).unwrap(), Some(receipt));
}

#[test]
fn a_missing_or_malformed_registry_never_proves_that_no_other_instance_needs_the_program() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("orch-connector");
    std::fs::write(&program, b"program").unwrap();
    let receipt = install(&program, &root.path().join("data"), Vec::new());
    let registry = root.path().join(".orch-connector.instances.json");
    let bytes = std::fs::read(&registry).unwrap();
    std::fs::remove_file(&registry).unwrap();
    assert!(Removing::begin(receipt.clone(), &program).is_err());
    std::fs::write(&registry, b"{}").unwrap();
    assert!(Removing::begin(receipt.clone(), &program).is_err());
    assert!(program.is_file());
    std::fs::write(&registry, bytes).unwrap();
    assert!(
        Removing::begin(receipt, &program)
            .unwrap()
            .may_remove_program()
    );
}

#[test]
fn the_exclusive_purge_lock_survives_removing_the_data_directory() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let running = InstanceLock::running(&data).unwrap();
    assert!(InstanceLock::removing(&data).is_err());
    drop(running);
    let removing = InstanceLock::removing(&data).unwrap();
    std::fs::remove_dir_all(&data).unwrap();
    assert!(InstanceLock::running(&data).is_err());
    std::fs::create_dir(&data).unwrap();
    assert!(InstanceLock::running(&data).is_err());
    drop(removing);
    assert!(InstanceLock::running(&data).is_ok());
}

#[cfg(unix)]
#[test]
fn a_link_cannot_replace_an_installation_receipt_or_make_the_size_measure_follow_borrowed_data() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let external = root.path().join("external");
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("keep"), vec![0; 8192]).unwrap();
    let link = data.join("link");
    std::os::unix::fs::symlink(&external, &link).unwrap();
    assert_eq!(
        uninstall::size(&data),
        Some(std::fs::symlink_metadata(&link).unwrap().len())
    );
    std::os::unix::fs::symlink(external.join("keep"), data.join(uninstall::RECEIPT)).unwrap();
    assert!(uninstall::inspect(&data).is_err());
    assert!(external.join("keep").is_file());
}

#[test]
fn a_parent_purge_keeps_both_running_and_uninstalled_nested_instances() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("orch-connector");
    std::fs::write(&program, b"program").unwrap();
    let outer = root.path().join("outer");
    let inner = outer.join("inner");
    let parent = install(&program, &outer, Vec::new());
    let child = install(&program, &inner, Vec::new());
    let removal = Removing::begin(parent.clone(), &program).unwrap();
    assert!(removal.purge_blocker().is_some());
    assert!(uninstall::finish_files(removal, true).is_err());
    assert!(inner.join(uninstall::RECEIPT).is_file());
    uninstall::finish_files(Removing::begin(child, &program).unwrap(), false).unwrap();
    let removal = Removing::begin(parent, &program).unwrap();
    assert_eq!(removal.other_instances(), 0);
    assert!(removal.purge_blocker().is_some());
    assert!(uninstall::finish_files(removal, true).is_err());
    assert!(inner.join(uninstall::RECEIPT).is_file());
    assert!(program.is_file());
}

#[test]
fn reviving_retained_data_keeps_the_shared_program_for_that_instance() {
    let root = tempfile::tempdir().unwrap();
    let program = root.path().join("orch-connector");
    std::fs::write(&program, b"program").unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    let first = install(&program, &a, Vec::new());
    let second = install(&program, &b, Vec::new());
    uninstall::finish_files(Removing::begin(second, &program).unwrap(), false).unwrap();
    assert!(uninstall::inspect(&b).unwrap().unwrap().uninstalled);
    uninstall::register_running(&b, &program, true).unwrap();
    assert!(!uninstall::inspect(&b).unwrap().unwrap().uninstalled);
    let first = Removing::begin(first, &program).unwrap();
    assert_eq!(first.other_instances(), 1);
    uninstall::finish_files(first, true).unwrap();
    assert!(program.is_file() && b.is_dir());
}
