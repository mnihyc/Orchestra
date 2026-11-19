//! The updater's protected storage snapshot is read-only input, never permission to delete server files.

use super::{Error, read};
use orch_api_contract::resource_facts::DependentRelation;
use orch_api_contract::storage::{
    OperatorStorageDependency, OperatorStorageInventory, StorageKind,
};
use std::path::Path;

/// The updater's first storage snapshot format.
const FORMAT: u32 = 1;
/// The same bounded allocation as an existing backup manifest; an oversized source is unavailable.
const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Published atomically beside the existing protected operator records.
pub const RECORD_FILE: &str = "storage.json";

/// Reads only protected metadata and an update hold; it never opens the directories the rows name.
pub fn inventory(dir: &Path, system: &str) -> Result<Option<OperatorStorageInventory>, Error> {
    let path = dir.join(RECORD_FILE);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Local("the operator storage metadata is unreadable")),
        Ok(_) => {}
    }
    let mut report: OperatorStorageInventory = serde_json::from_slice(&read(&path, MAX_BYTES)?)?;
    if report.format_version != FORMAT || report.system_id != system {
        return Err(Error::Local(
            "the operator storage metadata belongs to another installation or format",
        ));
    }
    let updating = match std::fs::symlink_metadata(dir.join("journal")) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return Err(Error::Local("the updater's current hold is unreadable")),
    };
    for item in &mut report.items {
        let row = &mut item.record;
        if !matches!(row.kind, StorageKind::Release | StorageKind::Dump)
            || row.resource.is_some()
            || row.place.is_some()
            || row.host.is_some()
            || row.machine.is_some()
            || row.owner.is_some()
        {
            return Err(Error::Local(
                "the operator storage metadata contains an unexpected resource",
            ));
        }
        // The shared facts service counts these source memberships after the caller's authority check.
        row.facts = None;
        if updating {
            row.operator_command = None;
            row.can_clean = false;
            row.blocker = Some("updater_busy".into());
            item.dependencies.push(OperatorStorageDependency {
                key: "updater/journal".into(),
                name: "update.sh recover".into(),
                relation: DependentRelation::Operation,
            });
        }
    }
    Ok(Some(report))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::resource_facts::{DependentGroup, FactSubject, ResourceFacts};
    use orch_api_contract::storage::{
        KeptRecord, KeptWhy, OperatorStorageInventory, OperatorStorageItem, StorageAvailability,
    };
    use orch_types::Time;

    fn report() -> OperatorStorageInventory {
        let at = Time::new("2026-10-01T12:00:00Z".parse().unwrap());
        let mut record = KeptRecord {
            id: "release:server:old".into(),
            accounting_key: "release:server:old".into(),
            kind: StorageKind::Release,
            name: "server/old".into(),
            resource: None,
            place: None,
            host: None,
            machine: None,
            owner: None,
            bytes: Some(1024),
            included_in: None,
            why: KeptWhy::Cache,
            since: Some(at),
            until: None,
            facts: None,
            observed_at: at,
            availability: StorageAvailability::Ready,
            can_clean: true,
            blocker: None,
            operator_command: Some("/opt/orch/deploy/update.sh prune".into()),
        };
        record.facts = Some(ResourceFacts {
            subject: FactSubject::ServerArtifact {
                artifact_kind: "release".into(),
                id: record.id.clone(),
            },
            as_of: at,
            dependents_complete: true,
            dependents: vec![DependentGroup {
                relation: DependentRelation::Operation,
                count: 999,
                visible: vec![],
                hidden: 999,
                until: None,
            }],
            uses: None,
        });
        OperatorStorageInventory {
            format_version: FORMAT,
            system_id: "isolated-system".into(),
            observed_at: at,
            items: vec![OperatorStorageItem {
                record,
                dependencies: vec![],
            }],
            issues: vec![],
        }
    }

    #[test]
    fn another_installations_metadata_is_refused_without_reading_its_named_files() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(RECORD_FILE),
            serde_json::to_vec(&report()).unwrap(),
        )
        .unwrap();
        assert!(inventory(directory.path(), "another-system").is_err());
        assert!(
            inventory(directory.path(), "isolated-system")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_live_update_blocks_commands_and_counts_its_hold_only_through_the_shared_source() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join(RECORD_FILE),
            serde_json::to_vec(&report()).unwrap(),
        )
        .unwrap();
        std::fs::write(directory.path().join("journal"), "step=restoring\n").unwrap();
        let result = inventory(directory.path(), "isolated-system")
            .unwrap()
            .unwrap();
        let item = &result.items[0];
        assert_eq!(item.record.operator_command, None);
        assert!(!item.record.can_clean);
        assert_eq!(item.record.blocker.as_deref(), Some("updater_busy"));
        assert_eq!(
            item.record.facts, None,
            "stored counters are not a second source"
        );
        assert_eq!(item.dependencies.len(), 1);
        assert_eq!(item.dependencies[0].relation, DependentRelation::Operation);
    }

    #[cfg(unix)]
    #[test]
    fn a_substituted_metadata_link_never_imports_another_file() {
        let directory = tempfile::tempdir().unwrap();
        let other = directory.path().join("other.json");
        std::fs::write(&other, serde_json::to_vec(&report()).unwrap()).unwrap();
        std::os::unix::fs::symlink(other, directory.path().join(RECORD_FILE)).unwrap();
        assert!(inventory(directory.path(), "isolated-system").is_err());
    }
}
