//! Retained-storage facts, physical-byte accounting and report breakdowns shared by every cleanup surface.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Duration;
use orch_api_contract::resources::HostRef;
use orch_api_contract::storage::{
    KeptRecord, KeptWhy, StorageAvailability, StorageBreakdown, StorageBreakdownRow,
    StorageDimension, StorageKind, StorageQuery, StorageTotals,
};
use orch_driver_contract::resources::{Resource, ResourceInventory, ResourceKind, ResourceRef};
use orch_types::Time;

/// Seven days is the shipped cache window; thirty days gives the next calendar-scale report group.
const FIRST_AGE_DAYS: i64 = 7;
const SECOND_AGE_DAYS: i64 = 30;

/// A fresh record starts with unknown ownership and size until its existing source supplies them.
#[must_use]
pub fn record(id: String, kind: StorageKind, name: String, observed_at: Time) -> KeptRecord {
    KeptRecord {
        artifact: None,
        physical_parts: Vec::new(),
        upload: None,
        accounting_key: id.clone(),
        id,
        kind,
        name,
        resource: None,
        place: None,
        host: None,
        machine: None,
        owner: None,
        bytes: None,
        included_in: None,
        why: KeptWhy::Unknown,
        since: None,
        until: None,
        facts: None,
        observed_at,
        availability: StorageAvailability::Unknown,
        can_clean: false,
        blocker: None,
        operator_command: None,
    }
}

/// A nonzero policy yields a date only when that date can be represented; a huge policy never overflows.
#[must_use]
pub fn cache_until(used: Option<Time>, days: i64) -> Option<Time> {
    if days <= 0 {
        return None;
    }
    let used = used?;
    let duration = Duration::try_days(days)?;
    used.get().checked_add_signed(duration).map(Time::new)
}

/// Translates all seven node kinds without using an implementation name or interpreting its opaque ids.
#[must_use]
pub const fn node_kind(kind: ResourceKind) -> StorageKind {
    match kind {
        ResourceKind::Unit => StorageKind::Unit,
        ResourceKind::Volume => StorageKind::Volume,
        ResourceKind::Network => StorageKind::Network,
        ResourceKind::Image => StorageKind::Image,
        ResourceKind::KeptLayer => StorageKind::KeptLayer,
        ResourceKind::Build => StorageKind::Build,
        ResourceKind::ToolVersion => StorageKind::ToolVersion,
    }
}

/// The common opaque key of one actual native inventory object.
#[must_use]
pub fn node_record_key(host: orch_types::Id, resource: &ResourceRef) -> String {
    format!("host:{}:{:?}:{}", host, resource.kind, resource.id)
}

/// Every native object uses the same retention fields; logical owners may enrich the facts afterwards.
#[must_use]
pub fn node_records(
    host: &HostRef,
    inventory: &ResourceInventory,
    days: i64,
    online: bool,
) -> Vec<KeptRecord> {
    node_records_selected(host, inventory, days, online, None)
}

/// Format selected native candidates while every native dependency still contributes safety facts.
#[must_use]
pub fn node_records_selected(
    host: &HostRef,
    inventory: &ResourceInventory,
    days: i64,
    online: bool,
    selected: Option<&BTreeSet<ResourceRef>>,
) -> Vec<KeptRecord> {
    let used: BTreeSet<_> = inventory
        .resources
        .iter()
        .flat_map(|item| item.depends_on.iter().cloned())
        .collect();
    inventory
        .resources
        .iter()
        .filter(|item| selected.is_none_or(|selected| selected.contains(&item.resource)))
        .map(|item| {
            let mut row = record(
                node_record_key(host.id, &item.resource),
                node_kind(item.resource.kind),
                item.name.clone(),
                inventory.observed_at,
            );
            row.host = Some(host.clone());
            row.bytes = item.size_bytes.filter(|bytes| *bytes >= 0);
            row.since = item.last_used_at;
            row.availability = if online {
                StorageAvailability::Ready
            } else {
                StorageAvailability::Away
            };
            row.can_clean = true;
            let needed = item.busy || used.contains(&item.resource);
            row.why = if needed {
                KeptWhy::InUse
            } else if !inventory.unavailable.is_empty() {
                KeptWhy::Unknown
            } else if matches!(
                item.resource.kind,
                ResourceKind::Image | ResourceKind::Build | ResourceKind::ToolVersion
            ) && item.last_used_at.is_some()
            {
                KeptWhy::Cache
            } else {
                KeptWhy::Unknown
            };
            if row.why == KeptWhy::Cache {
                row.until = cache_until(item.last_used_at, days);
            }
            if !online {
                row.blocker = Some("node_away".into());
            } else if needed && item.resource.kind != ResourceKind::Unit {
                row.blocker = Some("in_use".into());
            } else if !inventory.unavailable.is_empty() {
                row.blocker = Some("inventory_incomplete".into());
            }
            row
        })
        .collect()
}

/// The exact native object belonging to a report key, resolved only against this observed inventory.
#[must_use]
pub fn node_object<'a>(
    host: &HostRef,
    inventory: &'a ResourceInventory,
    id: &str,
) -> Option<&'a Resource> {
    inventory
        .resources
        .iter()
        .find(|item| node_record_key(host.id, &item.resource) == id)
}

/// Counts shared content once, and only while the larger enclosing record is absent from this subset.
#[must_use]
pub fn totals(records: &[KeptRecord]) -> StorageTotals {
    let parents: BTreeSet<_> = records
        .iter()
        .filter(|row| row.bytes.is_some())
        .map(|row| row.id.as_str())
        .collect();
    let mut groups = BTreeMap::<&str, Option<i64>>::new();
    for row in records.iter().filter(|row| {
        row.included_in
            .as_deref()
            .is_none_or(|parent| !parents.contains(parent))
    }) {
        if row.physical_parts.is_empty() {
            count_part(&mut groups, &row.accounting_key, row.bytes);
        } else {
            for part in &row.physical_parts {
                count_part(&mut groups, &part.key, part.bytes);
            }
        }
    }
    StorageTotals {
        items: i64::try_from(records.len()).unwrap_or(i64::MAX),
        bytes: groups
            .values()
            .flatten()
            .fold(0_i64, |sum, bytes| sum.saturating_add(*bytes)),
        unknown: i64::try_from(groups.values().filter(|bytes| bytes.is_none()).count())
            .unwrap_or(i64::MAX),
    }
}

fn count_part<'a>(groups: &mut BTreeMap<&'a str, Option<i64>>, key: &'a str, bytes: Option<i64>) {
    let bytes = bytes.filter(|bytes| *bytes >= 0);
    groups
        .entry(key)
        .and_modify(|held| {
            *held = match (*held, bytes) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
        })
        .or_insert(bytes);
}

/// Stable age filters use elapsed days, including their exact boundaries.
#[must_use]
pub fn age(row: &KeptRecord, at: Time) -> &'static str {
    let Some(since) = row.since else {
        return "unknown";
    };
    let days = (at.get() - since.get()).num_days().max(0);
    if days < FIRST_AGE_DAYS {
        "under_7_days"
    } else if days < SECOND_AGE_DAYS {
        "7_to_30_days"
    } else {
        "30_days_plus"
    }
}

fn word(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// One stable breakdown/filter key, including server data and unnamed values.
#[must_use]
pub fn dimension_key(row: &KeptRecord, by: StorageDimension, at: Time) -> String {
    match by {
        StorageDimension::Host => row
            .host
            .as_ref()
            .map(|host| host.id.to_string())
            .unwrap_or_else(|| {
                (if matches!(
                    row.kind,
                    StorageKind::Database
                        | StorageKind::Content
                        | StorageKind::Upload
                        | StorageKind::Export
                        | StorageKind::Snapshot
                        | StorageKind::Import
                        | StorageKind::SkillVersion
                        | StorageKind::Release
                        | StorageKind::Dump
                ) {
                    "server"
                } else {
                    "unknown"
                })
                .into()
            }),
        StorageDimension::Place => row
            .place
            .as_ref()
            .map_or_else(|| "unknown".into(), |place| place.id.to_string()),
        StorageDimension::Owner => row
            .owner
            .as_ref()
            .map_or_else(|| "unknown".into(), |owner| owner.id.to_string()),
        StorageDimension::Kind => word(&row.kind),
        StorageDimension::Why => word(&row.why),
        StorageDimension::Age => age(row, at).into(),
    }
}

/// Applies report filters after permissions, so figures and records cover the same visible set.
#[must_use]
pub fn matches(row: &KeptRecord, query: &StorageQuery, at: Time) -> bool {
    query.resource_id.is_none_or(|id| {
        let same = |resource: orch_access_contract::decisions::ObjectRef| {
            resource.id == id && query.resource_kind.is_none_or(|kind| kind == resource.kind)
        };
        row.resource.is_some_and(same)
            || (query.resource_kind == Some(orch_access_contract::permissions::ObjectKind::Host)
                && row.host.as_ref().is_some_and(|host| host.id == id))
            || row.facts.as_ref().is_some_and(|facts| {
                facts
                    .dependents
                    .iter()
                    .flat_map(|group| &group.visible)
                    .any(|dependent| dependent.object.is_some_and(same))
            })
    }) && query
        .host
        .as_deref()
        .is_none_or(|key| dimension_key(row, StorageDimension::Host, at) == key)
        && query
            .place
            .as_deref()
            .is_none_or(|key| dimension_key(row, StorageDimension::Place, at) == key)
        && query
            .owner
            .as_deref()
            .is_none_or(|key| dimension_key(row, StorageDimension::Owner, at) == key)
        && query.kind.is_none_or(|kind| row.kind == kind)
        && query.why.is_none_or(|why| row.why == why)
        && query.age.as_deref().is_none_or(|key| age(row, at) == key)
}

/// The same visible subset divided by host, kind, place, owner, age and reason.
#[must_use]
pub fn breakdowns(records: &[KeptRecord], at: Time) -> Vec<StorageBreakdown> {
    [
        StorageDimension::Host,
        StorageDimension::Kind,
        StorageDimension::Place,
        StorageDimension::Owner,
        StorageDimension::Age,
        StorageDimension::Why,
    ]
    .into_iter()
    .map(|by| {
        let mut groups = BTreeMap::<String, Vec<KeptRecord>>::new();
        for row in records {
            let key = dimension_key(row, by, at);
            groups.entry(key).or_default().push(row.clone());
        }
        let mut rows: Vec<_> = groups
            .into_iter()
            .map(|(key, members)| {
                let first = members.first();
                let name = match by {
                    StorageDimension::Host => first
                        .and_then(|row| row.host.as_ref())
                        .map(|host| host.name.clone()),
                    StorageDimension::Place => first
                        .and_then(|row| row.place.as_ref())
                        .and_then(|place| place.name.clone()),
                    StorageDimension::Owner => first
                        .and_then(|row| row.owner.as_ref())
                        .map(|owner| owner.display_name.clone()),
                    _ => None,
                }
                .unwrap_or_else(|| key.clone());
                let owner = if by == StorageDimension::Owner {
                    first.and_then(|row| row.owner.clone())
                } else {
                    None
                };
                let object = match by {
                    StorageDimension::Host => first.and_then(|row| row.host.as_ref()).map(|host| {
                        orch_access_contract::decisions::ObjectRef {
                            kind: orch_access_contract::permissions::ObjectKind::Host,
                            id: host.id,
                        }
                    }),
                    StorageDimension::Place => {
                        first.and_then(|row| row.place.as_ref()).and_then(|place| {
                            word(&place.kind).parse().ok().map(|kind| {
                                orch_access_contract::decisions::ObjectRef { kind, id: place.id }
                            })
                        })
                    }
                    _ => None,
                };
                StorageBreakdownRow {
                    key,
                    name,
                    object,
                    owner,
                    totals: totals(&members),
                }
            })
            .collect();
        rows.sort_by(|a, b| {
            b.totals
                .bytes
                .cmp(&a.totals.bytes)
                .then_with(|| a.key.cmp(&b.key))
        });
        StorageBreakdown { by, rows }
    })
    .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use orch_types::Id;

    #[test]
    fn shared_artifact_parts_count_once_with_raw_blobs_and_in_filtered_subsets() {
        use orch_api_contract::storage::StoragePart;
        let at = Time::new("2026-10-01T12:00:00Z".parse().unwrap());
        let mut first = record(
            "snapshot:first".into(),
            StorageKind::Snapshot,
            String::new(),
            at,
        );
        first.bytes = Some(17);
        first.physical_parts = vec![
            StoragePart {
                key: "blob:shared".into(),
                bytes: Some(10),
            },
            StoragePart {
                key: "blob:first".into(),
                bytes: Some(7),
            },
        ];
        let mut second = record(
            "import:second".into(),
            StorageKind::Import,
            String::new(),
            at,
        );
        second.bytes = Some(15);
        second.physical_parts = vec![
            StoragePart {
                key: "blob:shared".into(),
                bytes: Some(10),
            },
            StoragePart {
                key: "blob:second".into(),
                bytes: Some(5),
            },
        ];
        let mut raw = record(
            "blob:shared".into(),
            StorageKind::Content,
            String::new(),
            at,
        );
        raw.bytes = Some(10);
        assert_eq!(totals(&[first.clone(), second.clone(), raw]).bytes, 22);
        assert_eq!(totals(std::slice::from_ref(&first)).bytes, 17);
        assert_eq!(totals(std::slice::from_ref(&second)).bytes, 15);
        second.physical_parts.push(StoragePart {
            key: "blob:unmeasured".into(),
            bytes: None,
        });
        let combined = totals(&[first, second]);
        assert_eq!(combined.bytes, 22);
        assert_eq!(combined.unknown, 1);
    }

    #[test]
    fn selected_records_keep_dependencies_of_unselected_native_units() {
        let at = Time::new("2026-10-01T12:00:00Z".parse().unwrap());
        let volume = ResourceRef {
            kind: ResourceKind::Volume,
            id: "kept-volume".into(),
        };
        let inventory = ResourceInventory {
            observed_at: at,
            unavailable: Vec::new(),
            resources: vec![
                Resource {
                    resource: volume.clone(),
                    name: "Kept data".into(),
                    size_bytes: Some(11),
                    labels: Default::default(),
                    depends_on: Vec::new(),
                    aliases: Vec::new(),
                    last_used_at: Some(at),
                    busy: false,
                },
                Resource {
                    resource: ResourceRef {
                        kind: ResourceKind::Unit,
                        id: "unselected-user".into(),
                    },
                    name: "Unselected dependent".into(),
                    size_bytes: Some(2),
                    labels: Default::default(),
                    depends_on: vec![volume.clone()],
                    aliases: Vec::new(),
                    last_used_at: Some(at),
                    busy: false,
                },
            ],
        };
        let host = HostRef {
            id: Id::from_u128(1),
            name: "Host".into(),
            address: String::new(),
        };
        let selected = BTreeSet::from([volume]);
        let rows = node_records_selected(&host, &inventory, 7, true, Some(&selected));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Kept data");
        assert_eq!(rows[0].why, KeptWhy::InUse);
        assert_eq!(rows[0].blocker.as_deref(), Some("in_use"));
        assert_eq!(rows[0], node_records(&host, &inventory, 7, true)[0]);
    }
}
