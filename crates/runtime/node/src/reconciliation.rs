//! Reconcile this system's units without removing volumes.
//!
//! At start and every `[node] reconcile_secs` the node asks each driver for the units of its system (`list`)
//! and tells the system the real state of each runtime (`node.runtime`): running, stopped, failed with the exit
//! status, or absent. The system compares that with its rows and answers with commands: a row that says
//! `running` against a unit that is not is marked failed, a unit that runs against a row that says it should
//! not is stopped. An unrecorded unit stays visible in inventory for a reviewed cleanup.
//!
//! The node therefore **changes nothing here**: it looks and tells. What it tells is only what is new since
//! it last told (a full account at start, then differences), and a runtime a command is being carried out
//! for is left alone, because the command tells the system what became of it when it ends.
//!
//! A unit the node has no record of, which carries the system's label, is told with the note that this node
//! does not know it; the unit stays, and so does everything it stands on. The system's label has no one value
//! in the contract, so the node lists every value it has been given by a spec or a volume, and the name of the
//! system it is enrolled in until it has been given one.

use std::collections::BTreeSet;
use std::sync::Arc;

use orch_driver_contract::units::{UnitRef, UnitState, UnitStateKind};
use orch_link_contract::commands::NodeProbeUnit;
use orch_types::Id;

use crate::drivers::DriverHost;
use crate::facts::{Reports, detail_of, state_name};
use crate::images::Images;
use crate::ledger::Ledger;
use crate::runtimes::Busy;
use crate::settings::NodeSettings;

/// A unit a driver listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The driver that holds it.
    pub driver: String,
    /// The unit.
    pub unit: UnitRef,
}

/// Every unit each loaded driver holds for one of the `systems`.
pub async fn list_units(
    drivers: &DriverHost,
    systems: &[String],
) -> (Vec<Listed>, BTreeSet<String>) {
    let mut listed = Vec::new();
    let mut silent = BTreeSet::new();
    for (name, driver) in drivers.all() {
        for system in systems {
            match driver.contract().list(system).await {
                Ok(units) => listed.extend(units.into_iter().map(|unit| Listed {
                    driver: name.clone(),
                    unit,
                })),
                Err(error) => {
                    tracing::warn!(driver = %name, %error, "a driver could not list its units");
                    silent.insert(name.clone());
                }
            }
        }
    }
    (listed, silent)
}

/// The driver and unit of a runtime, found in what the drivers hold for these systems.
pub async fn locate(
    drivers: &DriverHost,
    systems: &[String],
    runtime_id: Id,
) -> Option<(String, UnitRef)> {
    let (listed, _) = list_units(drivers, systems).await;
    listed
        .into_iter()
        .find(|found| found.unit.runtime_id == runtime_id)
        .map(|found| (found.driver, found.unit))
}

/// A runtime as one pass of the reconciliation found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// The runtime.
    pub runtime_id: Id,
    /// The driver that holds its unit.
    pub driver: String,
    /// What the driver found its unit in; `Absent` when it holds none.
    pub state: UnitState,
    /// Whether the node has a record of the runtime.
    pub known: bool,
}

impl Observed {
    /// What is told of it: the state, and a sentence when there is something to say.
    #[must_use]
    pub fn told(&self) -> (&'static str, Option<String>) {
        let detail = detail_of(&self.state);
        let detail = if self.known {
            detail
        } else {
            Some(match detail {
                Some(own) => format!("this node has no record of it: {own}"),
                None => "this node has no record of it".to_owned(),
            })
        };
        (state_name(self.state.state), detail)
    }
}

/// Compares the units the drivers hold with what the node recorded, and tells the system.
pub struct Reconciler {
    drivers: Arc<DriverHost>,
    ledger: Arc<Ledger>,
    reports: Arc<Reports>,
    images: Arc<Images>,
    busy: Arc<Busy>,
    settings: NodeSettings,
}

impl std::fmt::Debug for Reconciler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reconciler").finish_non_exhaustive()
    }
}

impl Reconciler {
    /// A reconciliation for a node with these drivers and records.
    #[must_use]
    pub fn new(
        drivers: Arc<DriverHost>,
        ledger: Arc<Ledger>,
        reports: Arc<Reports>,
        images: Arc<Images>,
        busy: Arc<Busy>,
        settings: &NodeSettings,
    ) -> Self {
        Self {
            drivers,
            ledger,
            reports,
            images,
            busy,
            settings: settings.clone(),
        }
    }

    /// The values of the system label to list, in order.
    #[must_use]
    pub fn systems(&self) -> Vec<String> {
        let mut systems = self.ledger.systems();
        if !systems.contains(&self.settings.system_name) {
            systems.push(self.settings.system_name.clone());
        }
        systems
    }

    /// Every runtime the drivers hold a unit for, and every runtime the node recorded, with the state of
    /// its unit. A runtime of a driver that could not list is left out rather than reported absent.
    pub async fn observe(&self) -> Vec<Observed> {
        let (listed, silent) = list_units(&self.drivers, &self.systems()).await;
        let mut seen: BTreeSet<Id> = BTreeSet::new();
        let mut observed = Vec::new();
        for found in listed {
            if !seen.insert(found.unit.runtime_id) {
                continue;
            }
            let Some(driver) = self.drivers.get(&found.driver) else {
                continue;
            };
            let state = match driver.contract().inspect(&found.unit).await {
                Ok(state) => state,
                Err(error) => {
                    tracing::warn!(unit = %found.unit.name, %error, "a unit could not be looked at");
                    continue;
                }
            };
            observed.push(Observed {
                runtime_id: found.unit.runtime_id,
                known: self.ledger.get(found.unit.runtime_id).is_some(),
                driver: found.driver,
                state,
            });
        }
        for record in self.ledger.all() {
            if seen.contains(&record.runtime_id) || silent.contains(&record.driver) {
                continue;
            }
            // A driver that is not loaded cannot be asked, and a unit it holds is not said to be gone.
            if self.drivers.get(&record.driver).is_none() {
                continue;
            }
            observed.push(Observed {
                runtime_id: record.runtime_id,
                driver: record.driver,
                state: UnitState::of(UnitStateKind::Absent),
                known: true,
            });
        }
        observed
    }

    /// One pass: looks at every unit and tells the system what is new. Answers how many it told.
    ///
    /// A look takes time, and a command or the watch of an enrollment may act on a unit while it goes on. A
    /// runtime that has a command carrying on when the look ends is left to it; one the node told of since the
    /// look began is left too, for what the look saw may be what that telling changed, and the next pass
    /// sees the unit as it is.
    pub async fn pass(&self) -> usize {
        let mark = self.reports.mark();
        let observed = self.observe().await;
        let busy = self.busy.ids();
        let mut told = 0;
        for runtime in observed {
            if busy.contains(&runtime.runtime_id) {
                continue;
            }
            let (state, mut detail) = runtime.told();
            detail = self.ledger.mount_detail(runtime.runtime_id, detail);
            // A unit that runs an image that has fallen behind its tag says so, until it is updated.
            if let Some(why) = self.images.why_behind(runtime.runtime_id) {
                let note = format!("an update is available: {why}");
                detail = Some(detail.map_or_else(|| note.clone(), |own| format!("{own}; {note}")));
            }
            if self
                .reports
                .runtime_if_changed_since(mark, runtime.runtime_id, state, detail)
                .await
            {
                told += 1;
            }
        }
        told
    }

    /// The units in the form `node.probe` answers them in.
    pub async fn units(&self) -> Vec<NodeProbeUnit> {
        self.observe()
            .await
            .into_iter()
            .map(|runtime| NodeProbeUnit {
                runtime_id: runtime.runtime_id,
                state: state_name(runtime.state.state).to_owned(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_the_node_has_no_record_of_is_told_with_the_note_and_its_own_words() {
        let unit = Observed {
            runtime_id: orch_types::new_id(),
            driver: "memory".into(),
            state: UnitState {
                state: UnitStateKind::Failed,
                detail: Some("oom killed".into()),
                exit_code: Some(137),
                started_at: None,
                finished_at: None,
            },
            known: false,
        };
        let (state, detail) = unit.told();
        assert_eq!(state, "failed");
        assert_eq!(
            detail.as_deref(),
            Some("this node has no record of it: oom killed (exit status 137)")
        );
        let known = Observed {
            known: true,
            ..unit
        };
        assert_eq!(
            known.told().1.as_deref(),
            Some("oom killed (exit status 137)")
        );
    }
}
