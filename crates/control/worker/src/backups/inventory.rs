//! A typed restore inventory from the same identity- and hash-checked manifest as the backup record.

use super::{Error, validated_manifest};
use orch_api_contract::admin::{
    BackupInventoryGroup, BackupInventoryLocation, BackupRestoreList, DatabaseBackupRecord,
};
use orch_types::{Id, Json};
use std::path::Path;

fn text(value: &Json, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn id(value: &Json, key: &str) -> Result<Option<Id>, Error> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|_| Error::Local("the backup manifest has an invalid location identifier")),
    }
}

fn count(value: &Json, key: &str) -> Result<Option<u64>, Error> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value.as_u64().map(Some).ok_or(Error::Local(
            "the backup manifest has an invalid inventory count",
        )),
    }
}

fn location(value: &Json) -> Result<BackupInventoryLocation, Error> {
    if !value.is_object() {
        return Err(Error::Local(
            "the backup manifest has an invalid inventory location",
        ));
    }
    let location = text(value, "location");
    Ok(BackupInventoryLocation {
        object_id: id(value, "id")?,
        host_id: id(value, "host_id")?,
        machine_id: id(value, "machine_id")?,
        runtime_id: id(value, "runtime_id")?,
        driver: text(value, "driver"),
        storage: text(value, "storage"),
        mount: text(value, "mount"),
        location_known: location.is_some()
            && value.get("location_status").and_then(Json::as_str) == Some("known"),
        location,
        // This command inventories references, never separate host backups. Even a known path
        // or a copied arbitrary property cannot make it claim those bytes are backed up.
        copy_available: None,
    })
}

fn groups(value: &Json, included: bool) -> Result<Vec<BackupInventoryGroup>, Error> {
    let source = value
        .as_object()
        .ok_or(Error::Local("the backup manifest has no inventory object"))?;
    let mut groups = Vec::new();
    for (category, value) in source {
        if !value.is_object() || value.get("included").and_then(Json::as_bool) != Some(included) {
            return Err(Error::Local(
                "the backup manifest has an invalid coverage category",
            ));
        }
        let mut locations = Vec::new();
        if value.get("location").is_some() {
            locations.push(location(value)?);
        }
        if let Some(entries) = value.get("entries") {
            let entries = entries.as_array().ok_or(Error::Local(
                "the backup manifest has an invalid location list",
            ))?;
            for entry in entries {
                locations.push(location(entry)?);
            }
        }
        groups.push(BackupInventoryGroup {
            category: category.clone(),
            count: count(value, "count")?,
            environments_requiring_inspection: count(value, "environments_requiring_inspection")?,
            reason: text(value, "reason").unwrap_or_default(),
            locations,
        });
    }
    groups.sort_by(|left, right| left.category.cmp(&right.category));
    Ok(groups)
}

/// Reads only the published protected manifest, bounded by the ordinary manifest reader. A newer,
/// older or unrelated manifest cannot supply the list for the backup the caller currently knows.
pub fn read(
    dir: &Path,
    system_id: &str,
    expected: Option<&DatabaseBackupRecord>,
) -> Result<Option<BackupRestoreList>, Error> {
    let (publication, manifest) = validated_manifest(dir, system_id)?;
    let (Some(success), Some(manifest)) = (publication.success, manifest) else {
        return Ok(None);
    };
    let Some(expected) = expected else {
        return Err(Error::Local(
            "the completed backup record changed; refresh it before reading the restore list",
        ));
    };
    let mut expected = expected.clone();
    // A later failed attempt changes no dump identity and cannot make its restore list disappear.
    expected
        .last_failure
        .clone_from(&success.record.last_failure);
    if expected != success.record {
        return Err(Error::Local(
            "the backup manifest differs from the last verified backup; refresh the record",
        ));
    }
    Ok(Some(BackupRestoreList {
        backup: success.record,
        manifest_name: success.manifest_file,
        manifest_hash: success.manifest_hash,
        included: groups(&manifest.included, true)?,
        excluded: groups(&manifest.excluded, false)?,
    }))
}
