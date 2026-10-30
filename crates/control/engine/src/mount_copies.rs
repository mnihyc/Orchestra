//! What a move, fork or switch plan decides for each mount, and the copies of a mount's data its job makes.
//!
//! A plan that takes an environment elsewhere offers, for each mount, to detach it, to mount it as it is where the
//! target machine has a location for it, or to copy its data there, and preselects what the mount's manager chose
//! as its default. A copy reads the source machine's location whole, where it lies, and lands in the target
//! machine's location or in a new volume with a name of that machine's own, before the unit that uses it is made.
//! What a copy displaces is kept aside, attached nowhere, until somebody purges it. A data kind selects only the
//! optional content comparison of an attached read-only synced location; it changes no mount behavior.

use chrono::{DateTime, Utc};
use orch_api_contract::forks::{
    CopyInto, CopyUnavailable, MountChoice, PlanBlocker, PlanCopyHolds, PlanCopyLocation,
    PlanMountCopy, PlanMountFingerprints,
};
use orch_api_contract::mounts::{EnvMount, MountDataKind, MountFate, MountLocationKind, MountMode};
use orch_authz::Principal;
use orch_authz::resources::{Locating, LocationAsk, LocationKind, may_add_location};
use orch_authz::usable::Policies;
use orch_db::Conn;
use orch_db::queries::resources::mounts as rows;
use orch_db::queries::{jobs, storage};
use orch_driver_contract::volumes::VolumeMeasure;
use orch_link_contract::commands::{
    MountData, NodeMountExport, NodeMountFingerprint, NodeMountImport, NodeVolumeEnsure,
    NodeVolumeRemove, SerialKey, VolumeArchive,
};
use orch_types::{Hash, Id, Json, new_id};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;

use crate::commands::Outgoing;
use crate::jobs::runner::{Failure, Job, Stop};
use crate::mounts::Fated;
use crate::{Engine, EngineError};

/// How many trailing hexadecimal digits of an identifier name a machine or a copy in a volume's name: the last
/// ones, since the first twelve of an identifier are the millisecond it was made in.
const NAME_DIGITS: usize = 12;

/// A mount's data on one machine, named exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DataRef {
    /// `path` or `volume`.
    pub kind: String,
    /// The directory, or the volume's name.
    pub source: String,
    /// A volume's storage; none for the host's default.
    pub storage: Option<String>,
}

impl DataRef {
    fn of(location: &rows::LocationRow) -> Self {
        Self {
            kind: location.kind.clone(),
            source: location.source.clone(),
            storage: location.storage.clone(),
        }
    }

    fn volume(name: String) -> Self {
        Self {
            kind: "volume".into(),
            source: name,
            storage: None,
        }
    }

    /// The data as the machine that keeps it is told of it.
    #[must_use]
    pub fn link(&self) -> MountData {
        if self.kind == "path" {
            MountData::Path {
                path: self.source.clone(),
            }
        } else {
            MountData::Volume {
                name: self.source.clone(),
                storage: self.storage.clone(),
            }
        }
    }
}

/// What the target machine's location held, which the job keeps aside before the copy lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptDecision {
    /// The location as it was.
    pub previous: DataRef,
    /// For a replace: the volume the job makes and fills with what the location held, which is then what is kept;
    /// otherwise the previous location itself is kept where it is.
    pub copied_into: Option<DataRef>,
    /// How many files it held, where measured.
    pub file_count: Option<i64>,
    /// How many bytes.
    pub total_bytes: Option<i64>,
    /// The hint the location had.
    pub data_kind: Option<String>,
}

/// One copy of a mount's data that a confirmed plan makes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyDecision {
    /// The mount.
    pub mount_id: Id,
    /// The machine its data is read on.
    pub source_machine: Id,
    /// The driver that reads it there.
    pub source_driver: String,
    /// Where it is read.
    pub from: DataRef,
    /// The machine it lands on.
    pub target_machine: Id,
    /// The driver that writes it there.
    pub target_driver: String,
    /// The volume it is unpacked into.
    pub to: DataRef,
    /// Whether the job makes that volume.
    pub made: bool,
    /// What the target machine's location held, kept aside; none when it had none or held nothing.
    pub kept: Option<KeptDecision>,
    /// How many files the plan measured.
    pub file_count: i64,
    /// How many bytes.
    pub total_bytes: i64,
    /// The hint the target location has afterwards.
    pub data_kind: Option<String>,
}

/// What a confirmed plan does with the mounts of its result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountDecisions {
    /// Mounts the result does not attach, though its machine may have a location for them.
    #[serde(default)]
    pub detached: Vec<Id>,
    /// Data copied before the result attaches it.
    #[serde(default)]
    pub copies: Vec<CopyDecision>,
}

impl MountDecisions {
    /// The locations the copies make or fill on `machine`, as the rows a resolution reads in place of that
    /// machine's own.
    #[must_use]
    pub fn planned(
        &self,
        machine: Option<Id>,
        by: Id,
        at: DateTime<Utc>,
    ) -> Vec<rows::LocationRow> {
        self.copies
            .iter()
            .filter(|copy| Some(copy.target_machine) == machine)
            .map(|copy| rows::LocationRow {
                mount_id: copy.mount_id,
                machine_id: copy.target_machine,
                host_id: None,
                host_name: None,
                kind: copy.to.kind.clone(),
                source: copy.to.source.clone(),
                storage: copy.to.storage.clone(),
                data_kind: copy.data_kind.clone(),
                created_by: by,
                created_at: at,
            })
            .collect()
    }

    /// Bytes and files the copies carry, for an estimate.
    #[must_use]
    pub fn totals(&self) -> (i64, i64) {
        self.copies.iter().fold((0, 0), |(files, bytes), copy| {
            (
                files.saturating_add(copy.file_count),
                bytes.saturating_add(copy.total_bytes),
            )
        })
    }
}

/// What a plan may do with the mounts of its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    /// The result stays in an environment used as it stands: each mount stays as it is.
    Unchanged,
    /// The result goes elsewhere and its job detaches and copies as the plan says.
    Everything,
    /// The result goes elsewhere by a kind of move whose job neither detaches nor copies: each mount as it is.
    AsIsOnly,
}

/// The decisions a job's plan carries; none for a job planned before plans decided mounts.
pub(crate) fn of_plan(plan: &Json) -> Result<MountDecisions, EngineError> {
    plan.get("mount_decisions")
        .filter(|value| !value.is_null())
        .map_or_else(
            || Ok(MountDecisions::default()),
            |value| serde_json::from_value(value.clone()).map_err(EngineError::from),
        )
}

/// Switches the detached mounts off in an environment's own configuration, so that whatever makes its unit again
/// leaves them detached too. Switching a mount off asks nothing of whoever wrote it.
pub(crate) fn detach_in(config: &mut Json, detached: &[Id]) {
    if detached.is_empty() {
        return;
    }
    let Some(config) = config.as_object_mut() else {
        return;
    };
    let mounts = config
        .entry(orch_settings_contract::registry::keys::ENV_MOUNTS)
        .or_insert_with(|| serde_json::json!({}));
    if !mounts.is_object() {
        *mounts = serde_json::json!({});
    }
    if let Some(mounts) = mounts.as_object_mut() {
        for id in detached {
            mounts.insert(id.to_string(), serde_json::Value::Bool(false));
        }
    }
}

/// What a plan asks of the mounts, and what it knows of the two sides.
pub struct Ask<'a> {
    /// The request's choices.
    pub choices: &'a [MountChoice],
    /// Which explicit content comparison this plan starts or continues; none never reads contents.
    pub check_fingerprints: Option<Id>,
    /// What may be chosen.
    pub offer: Offer,
    /// The mounts as the source environment has them.
    pub source: &'a [EnvMount],
    /// The mounts as the result would have them with the locations its machine has now.
    pub target: &'a [EnvMount],
    /// The machine the source's data is on.
    pub source_machine: Option<Id>,
    /// The machine the result runs on.
    pub target_machine: Option<Id>,
    /// Who plans it, for what of the target's locations they may see.
    pub principal: &'a Principal,
    /// When.
    pub at: DateTime<Utc>,
}

/// What a plan says of the mounts, and what its job is to do with them.
pub struct Decided {
    /// One for each mount of either side.
    pub fated: Vec<Fated>,
    /// What the job does.
    pub decisions: MountDecisions,
    /// What stops the plan.
    pub blockers: Vec<PlanBlocker>,
}

fn blocker(code: &str, message: String) -> PlanBlocker {
    PlanBlocker {
        code: code.into(),
        message,
        missing: None,
    }
}

fn fate_of(word: &str) -> MountFate {
    match word {
        "detach" => MountFate::Detach,
        "copy" => MountFate::Copy,
        _ => MountFate::AsIs,
    }
}

fn data_kind_of(word: Option<&str>) -> Option<MountDataKind> {
    match word? {
        "own" => Some(MountDataKind::Own),
        "shared" => Some(MountDataKind::Shared),
        "synced" => Some(MountDataKind::Synced),
        _ => None,
    }
}

fn tail(id: Id) -> String {
    let text = id.simple().to_string();
    text[text.len() - NAME_DIGITS..].to_owned()
}

/// The name of a volume a copy makes for a mount on a machine: the machine's own, and, where that name is taken
/// already, the copy's too.
fn made_name(mount: Id, machine: Id, seed: Id, taken: &[String]) -> String {
    let name = format!("orch-mount-{mount}-{}", tail(machine));
    if taken.contains(&name) {
        format!("{name}-{}", tail(seed))
    } else {
        name
    }
}

/// The volume a replace fills with what the location held, so that it is kept.
fn kept_name(mount: Id, machine: Id, seed: Id) -> String {
    format!("orch-mount-{mount}-{}-kept-{}", tail(machine), tail(seed))
}

/// Starts or reuses a background count; a pending source size is an estimate, never a timed refusal.
async fn measure(
    engine: &Engine,
    machine: Id,
    driver: &str,
    mount: Id,
    data: &DataRef,
    by: Id,
) -> Result<(Option<VolumeMeasure>, orch_api_contract::forks::Measurement), String> {
    let count = engine
        .measure_in_background(
            machine,
            orch_link_contract::commands::CommandKind::NodeMountMeasure,
            json!({"driver":driver,"mount_id":mount,"data":data.link()}),
            json!({"mount_id":mount}),
            by,
        )
        .await
        .map_err(|error| error.to_string())?;
    let measured = count
        .result
        .map(serde_json::from_value::<VolumeMeasure>)
        .transpose()
        .map_err(|error| error.to_string())?;
    if measured.is_some_and(|value| value.file_count < 0 || value.total_bytes < 0) {
        return Err("the host returned an invalid measurement".into());
    }
    if let (Some(measured), Some(at)) = (measured.as_ref(), count.reference.measured_at) {
        let mut conn = engine
            .db()
            .acquire()
            .await
            .map_err(|error| error.to_string())?;
        sqlx::query("UPDATE mount_locations SET last_bytes=$3,size_at=$4 WHERE mount_id=$1 AND machine_id=$2 AND kind=$5 AND source=$6")
            .bind(mount).bind(machine).bind(measured.total_bytes).bind(at.get()).bind(&data.kind).bind(&data.source)
            .execute(&mut *conn).await.map_err(|error|error.to_string())?;
    }
    Ok((measured, count.reference))
}

/// Whether both hosts can read and write whole locations, asked once for a plan.
async fn both_carry(engine: &Engine, source: (Id, &str), target: (Id, &str)) -> Result<(), String> {
    for (machine, driver) in [source, target] {
        crate::volume_carry::check_target(engine, machine, driver)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

async fn fingerprint(
    engine: &Engine,
    request_id: Id,
    source: (Id, &str, &rows::LocationRow),
    target: (Id, &str, &rows::LocationRow),
    by: Id,
) -> Result<
    (
        Option<PlanMountFingerprints>,
        orch_api_contract::forks::Measurement,
    ),
    EngineError,
> {
    let body = |driver: &str, location: &rows::LocationRow| NodeMountFingerprint {
        driver: driver.into(),
        mount_id: location.mount_id,
        data: DataRef::of(location).link(),
        work: None,
    };
    let count = engine
        .measure_in_background(
            source.0,
            orch_link_contract::commands::CommandKind::NodeMountFingerprint,
            serde_json::to_value(crate::jobs::measurement::FingerprintInput {
                request_id,
                source: body(source.1, source.2),
                target_machine: target.0,
                target: body(target.1, target.2),
            })?,
            json!({"mount_id":source.2.mount_id}),
            by,
        )
        .await?;
    let result = count
        .result
        .map(serde_json::from_value::<PlanMountFingerprints>)
        .transpose()?;
    Ok((result, count.reference))
}

/// Decides each mount's fate for a plan: the request's choice, else its manager's default where that can be done
/// there, else as it is; measures what a copy carries and what it would land on.
pub async fn decide(
    engine: &Engine,
    conn: &mut Conn,
    ask: &Ask<'_>,
) -> Result<Decided, EngineError> {
    decide_from(engine, conn, ask, None).await
}

/// Plans an authorized host location's move with the same new-location copy used by environment moves.
pub async fn decide_location_copy(
    engine: &Engine,
    conn: &mut Conn,
    principal: &Principal,
    source: &rows::LocationRow,
    target: Id,
    at: DateTime<Utc>,
) -> Result<Decided, EngineError> {
    use orch_api_contract::mounts::{EnvMountState, MountRef};
    let row = rows::mounts(conn, &[source.mount_id])
        .await?
        .pop()
        .ok_or_else(|| EngineError::unreadable("the mount location has no live mount"))?;
    let mount = EnvMount {
        mount: MountRef {
            id: row.id,
            name: row.name,
            address: String::new(),
        },
        state: EnvMountState::Attached,
        reason: None,
        path: Some(row.target),
        mode: MountMode::ReadOnly,
        asked: MountMode::ReadOnly,
        enforced: true,
        from: None,
    };
    let choices = [MountChoice {
        mount_id: source.mount_id,
        fate: MountFate::Copy,
        into: Some(CopyInto::NewLocation),
    }];
    decide_from(
        engine,
        conn,
        &Ask {
            choices: &choices,
            check_fingerprints: None,
            offer: Offer::Everything,
            source: std::slice::from_ref(&mount),
            target: &[],
            source_machine: Some(source.machine_id),
            target_machine: Some(target),
            principal,
            at,
        },
        Some(source),
    )
    .await
}

async fn decide_from(
    engine: &Engine,
    conn: &mut Conn,
    ask: &Ask<'_>,
    source: Option<&rows::LocationRow>,
) -> Result<Decided, EngineError> {
    let mut ids: Vec<Id> = ask
        .source
        .iter()
        .chain(ask.target)
        .map(|mount| mount.mount.id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mounts = rows::mounts(conn, &ids).await?;
    let mut locations = rows::locations(conn, &ids).await?;
    if let Some(source) = source {
        locations
            .retain(|row| row.mount_id != source.mount_id || row.machine_id != source.machine_id);
        locations.push(source.clone());
    }
    let kept = rows::kept(conn, &ids).await?;
    let mut blockers = Vec::new();
    if ask
        .choices
        .iter()
        .any(|choice| !ids.contains(&choice.mount_id))
    {
        blockers.push(blocker(
            "mount_not_planned",
            "The request names a mount that neither environment has.".into(),
        ));
    }
    let different = ask.offer == Offer::Everything
        && ask.source_machine.is_some()
        && ask.target_machine.is_some()
        && ask.source_machine != ask.target_machine;
    let source_driver = match ask.source_machine {
        Some(machine) => rows::data_driver(conn, machine)
            .await?
            .map(|(name, _)| name),
        None => None,
    };
    let target_driver = match ask.target_machine {
        Some(machine) => rows::data_driver(conn, machine)
            .await?
            .map(|(name, _)| name),
        None => None,
    };
    // Whether whole locations can be read and written at all is asked of the two hosts once, and only when a
    // mount could otherwise be copied.
    let readable = |id: Id| {
        ask.source
            .iter()
            .any(|mount| mount.mount.id == id && mount.reason.is_none())
            && locations.iter().any(|location| {
                location.mount_id == id && Some(location.machine_id) == ask.source_machine
            })
    };
    let carry = match (
        ask.source_machine,
        ask.target_machine,
        &source_driver,
        &target_driver,
    ) {
        (Some(source), Some(target), Some(read), Some(write))
            if different && ids.iter().any(|id| readable(*id)) =>
        {
            both_carry(engine, (source, read.as_str()), (target, write.as_str())).await
        }
        _ => {
            Err("only hosts whose drivers copy whole volumes read and write a mount's data".into())
        }
    };
    let seed = new_id();
    let mut fated = Vec::new();
    let mut decisions = MountDecisions::default();
    for id in ids {
        let Some(row) = mounts.iter().find(|row| row.id == id) else {
            continue;
        };
        let default_fate = fate_of(&row.default_fate);
        let source_location = locations.iter().find(|location| {
            location.mount_id == id && Some(location.machine_id) == ask.source_machine
        });
        let target_location = locations.iter().find(|location| {
            location.mount_id == id && Some(location.machine_id) == ask.target_machine
        });
        let copy_unavailable = if ask.offer == Offer::Unchanged {
            Some(CopyUnavailable::Unchanged)
        } else if ask.offer == Offer::AsIsOnly {
            Some(CopyUnavailable::Unsupported)
        } else if !different {
            Some(CopyUnavailable::SameMachine)
        } else if !readable(id) {
            Some(CopyUnavailable::NotAttached)
        } else if carry.is_err() {
            Some(CopyUnavailable::Unsupported)
        } else {
            None
        };
        let mut fates = if ask.offer == Offer::Everything {
            vec![MountFate::Detach, MountFate::AsIs]
        } else {
            vec![MountFate::AsIs]
        };
        if copy_unavailable.is_none() {
            fates.push(MountFate::Copy);
        }
        let choice = ask.choices.iter().find(|choice| choice.mount_id == id);
        let fate = match choice {
            Some(choice) => {
                if !fates.contains(&choice.fate) {
                    blockers.push(match (choice.fate, copy_unavailable) {
                        (MountFate::Copy, Some(why)) => blocker(
                            "mount_copy_unavailable",
                            format!(
                                "{} cannot be copied there: {}.",
                                row.name,
                                match why {
                                    CopyUnavailable::Unchanged =>
                                        "the result stays in an environment used as it stands"
                                            .to_owned(),
                                    CopyUnavailable::SameMachine =>
                                        "its data is on that machine already".to_owned(),
                                    CopyUnavailable::NotAttached =>
                                        "the source does not have it attached".to_owned(),
                                    CopyUnavailable::Unsupported =>
                                        carry.clone().err().unwrap_or_default(),
                                }
                            ),
                        ),
                        _ => blocker(
                            "mount_fate_unavailable",
                            format!(
                                "{} stays as it is: {}.",
                                row.name,
                                if ask.offer == Offer::Unchanged {
                                    "the result stays in an environment used as it stands"
                                } else {
                                    "a move between these machines carries each mount as it is"
                                }
                            ),
                        ),
                    });
                }
                choice.fate
            }
            None if fates.contains(&default_fate) => default_fate,
            None => MountFate::AsIs,
        };
        if fate == MountFate::Detach && fates.contains(&MountFate::Detach) {
            decisions.detached.push(id);
        }
        let mut copy = None;
        if let (
            MountFate::Copy,
            None,
            Some(from),
            Some(read),
            Some(write),
            Some(source),
            Some(target),
        ) = (
            fate,
            copy_unavailable,
            source_location,
            &source_driver,
            &target_driver,
            ask.source_machine,
            ask.target_machine,
        ) {
            let from = DataRef::of(from);
            let measured = measure(engine, source, read, id, &from, ask.principal.user()).await;
            if let Err(why) = &measured {
                blockers.push(blocker(
                    "mount_copy_unmeasured",
                    format!("The source host could not measure the data of {}; no copy has started: {why}", row.name),
                ));
            }
            let (measured, measurement) =
                measured.map_or((None, None), |(value, reference)| (value, Some(reference)));
            let taken: Vec<String> = locations
                .iter()
                .filter(|location| location.machine_id == target)
                .map(|location| location.source.clone())
                .chain(
                    kept.iter()
                        .filter(|data| data.machine_id == target)
                        .map(|data| data.source.clone()),
                )
                .collect();
            let into = choice.and_then(|choice| choice.into);
            let made = DataRef::volume(if into == Some(CopyInto::NewLocation) {
                // A new location is distinct even before another accepted copy publishes its row.
                format!("orch-mount-{id}-{}-{}", tail(target), tail(seed))
            } else {
                made_name(id, target, seed, &taken)
            });
            let shown = match target_location {
                Some(location) => matches!(
                    may_add_location(
                        conn,
                        ask.principal,
                        &LocationAsk {
                            mount: id,
                            machine: target,
                            kind: if location.kind == "volume" {
                                LocationKind::Volume
                            } else {
                                LocationKind::Path
                            },
                            storage: location.storage.clone(),
                        },
                        &Policies::default(),
                    )
                    .await?,
                    Locating::Allowed
                ),
                None => true,
            };
            // Where it lands, what is kept aside, and what the person still has to say.
            let (to, landed_made, kept_aside, holds, replace) = match target_location {
                None => (made, true, None, None, false),
                Some(location) => {
                    let count = measure(
                        engine,
                        target,
                        write,
                        id,
                        &DataRef::of(location),
                        ask.principal.user(),
                    )
                    .await
                    .ok();
                    let (held, target_measurement) =
                        count.map_or((None, None), |(value, reference)| (value, Some(reference)));
                    if held.is_none() && into != Some(CopyInto::NewLocation) {
                        blockers.push(blocker("measurement_pending", format!("Waiting for the count of {} on the target before deciding what a copy replaces.", row.name)));
                    }
                    let holds_data =
                        held.is_none_or(|held| held.file_count > 0 || held.total_bytes > 0);
                    let in_use = !rows::holders_on(conn, id, target).await?.is_empty();
                    let replace = location.kind == "volume" && !in_use;
                    let holds = holds_data.then(|| PlanCopyHolds {
                        measurement: target_measurement,
                        measured: held.is_some(),
                        file_count: held.map_or(0, |held| held.file_count),
                        total_bytes: held.map_or(0, |held| held.total_bytes),
                    });
                    let previous = KeptDecision {
                        previous: DataRef::of(location),
                        copied_into: None,
                        file_count: held.map(|held| held.file_count),
                        total_bytes: held.map(|held| held.total_bytes),
                        data_kind: location.data_kind.clone(),
                    };
                    if holds_data {
                        match into {
                            Some(CopyInto::Replace) if replace => (
                                DataRef::of(location),
                                false,
                                Some(KeptDecision {
                                    copied_into: Some(DataRef::volume(kept_name(id, target, seed))),
                                    ..previous
                                }),
                                holds,
                                replace,
                            ),
                            Some(CopyInto::Replace) => {
                                blockers.push(blocker(
                                    "mount_copy_replace_unavailable",
                                    format!(
                                        "The data of {} on that machine cannot be replaced: {}. Copy it into a new location instead.",
                                        row.name,
                                        if location.kind == "volume" {
                                            "an environment there has it attached"
                                        } else {
                                            "it is a directory Orch did not make"
                                        }
                                    ),
                                ));
                                (made, true, Some(previous), holds, replace)
                            }
                            Some(CopyInto::NewLocation) => {
                                (made, true, Some(previous), holds, replace)
                            }
                            None => {
                                if held.is_some() {
                                    blockers.push(blocker(
                                    "mount_copy_into_required",
                                    format!(
                                        "{} already has data on that machine: choose to replace it or to keep both.",
                                        row.name
                                    ),
                                ));
                                }
                                (made, true, Some(previous), holds, replace)
                            }
                        }
                    } else if replace && into != Some(CopyInto::NewLocation) {
                        // An empty volume that nothing uses takes the copy as it is; nothing is lost.
                        (DataRef::of(location), false, None, None, replace)
                    } else {
                        (made, true, Some(previous), None, replace)
                    }
                }
            };
            let file_count = measured.map_or(0, |measured| measured.file_count);
            let total_bytes = measured.map_or(0, |measured| measured.total_bytes);
            copy = Some(PlanMountCopy {
                measurement,
                measured: measured.is_some(),
                file_count,
                total_bytes,
                location: PlanCopyLocation {
                    made: landed_made,
                    kind: if to.kind == "path" {
                        MountLocationKind::Path
                    } else {
                        MountLocationKind::Volume
                    },
                    source: (landed_made || shown).then(|| to.source.clone()),
                    storage: to.storage.clone(),
                },
                holds,
                into,
                replace,
            });
            decisions.copies.push(CopyDecision {
                mount_id: id,
                source_machine: source,
                source_driver: read.clone(),
                from,
                target_machine: target,
                target_driver: write.clone(),
                data_kind: if landed_made {
                    None
                } else {
                    target_location.and_then(|location| location.data_kind.clone())
                },
                to,
                made: landed_made,
                kept: kept_aside,
                file_count,
                total_bytes,
            });
        }
        let fingerprints_available = fate == MountFate::AsIs
            && readable(id)
            && target_location
                .is_some_and(|location| location.data_kind.as_deref() == Some("synced"))
            && ask.target.iter().any(|mount| {
                mount.mount.id == id && mount.reason.is_none() && mount.mode == MountMode::ReadOnly
            })
            && source_driver.is_some()
            && target_driver.is_some();
        let (fingerprints, fingerprint_measurement) = match (
            ask.check_fingerprints.filter(|_| fingerprints_available),
            source_location,
            target_location,
            ask.source_machine,
            ask.target_machine,
            &source_driver,
            &target_driver,
        ) {
            (
                Some(request),
                Some(from),
                Some(to),
                Some(source),
                Some(target),
                Some(read),
                Some(write),
            ) => {
                let (result, reference) = fingerprint(
                    engine,
                    request,
                    (source, read, from),
                    (target, write, to),
                    ask.principal.user(),
                )
                .await?;
                (result, Some(reference))
            }
            _ => (None, None),
        };
        fated.push(Fated {
            mount_id: id,
            fate,
            default_fate,
            fates,
            copy_unavailable,
            copy,
            data_kind: data_kind_of(
                source_location.and_then(|location| location.data_kind.as_deref()),
            ),
            fingerprints_available,
            fingerprints,
            fingerprint_measurement,
        });
    }
    Ok(Decided {
        fated,
        decisions,
        blockers,
    })
}

// ---------------------------------------------------------------- the job's part

/// What a job remembers of its copies, so that a job taken up again does none of it twice.
mod done {
    /// The archives of what each copy carries, by mount.
    pub const ARCHIVES: &str = "mount_copy_archives";
    /// The archives of what a location held before a copy landed in it, by mount.
    pub const KEPT_ARCHIVES: &str = "mount_copy_kept_archives";
    /// The mounts whose copy began to be unpacked where a location's data is replaced.
    pub const UNPACKING: &str = "mount_copy_unpacking";
    /// The blob parts held while the job runs.
    pub const PINS: &str = "mount_copy_pins";
}

fn copy_failed(message: impl Into<String>) -> Stop {
    Stop::Failed(Failure::new("mount_copy_failed", message))
}

fn member<T: serde::de::DeserializeOwned + Default>(plan: &Json, key: &str) -> Result<T, Stop> {
    plan.get(key).filter(|value| !value.is_null()).map_or_else(
        || Ok(T::default()),
        |value| {
            serde_json::from_value(value.clone())
                .map_err(|error| Stop::from(EngineError::from(error)))
        },
    )
}

fn labels(mount: Id, job: Id) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("orch.mount_id".to_owned(), mount.to_string()),
        ("orch.copy_job".to_owned(), job.to_string()),
    ])
}

async fn ensure(
    job: &Job,
    machine: Id,
    driver: &str,
    mount: Id,
    volume: &DataRef,
) -> Result<(), Stop> {
    job.command(
        Outgoing::new(
            crate::ids::derived(
                "node.volume.ensure",
                &[
                    job.id().as_bytes(),
                    mount.as_bytes(),
                    volume.source.as_bytes(),
                ],
            ),
            machine,
            SerialKey::Volume(volume.source.clone()),
            &NodeVolumeEnsure {
                driver: driver.into(),
                name: volume.source.clone(),
                storage: volume.storage.clone(),
                labels: labels(mount, job.id()),
            },
        )
        .map_err(EngineError::from)?,
    )
    .await?;
    Ok(())
}

async fn remove(
    job: &Job,
    machine: Id,
    driver: &str,
    mount: Id,
    volume: &DataRef,
) -> Result<(), Stop> {
    job.command_to_undo(
        Outgoing::new(
            crate::ids::derived(
                "node.volume.remove",
                &[
                    job.id().as_bytes(),
                    mount.as_bytes(),
                    volume.source.as_bytes(),
                ],
            ),
            machine,
            SerialKey::Volume(volume.source.clone()),
            &NodeVolumeRemove {
                driver: driver.into(),
                name: volume.source.clone(),
                storage: volume.storage.clone(),
                labels: labels(mount, job.id()),
            },
        )
        .map_err(EngineError::from)?,
    )
    .await?;
    Ok(())
}

/// Archives a mount's data where it lies, once: an archive the job made before is used again. Its parts are held
/// until the job ends.
async fn archived(
    job: &Job,
    key: &str,
    mount: Id,
    machine: Id,
    driver: &str,
    data: &DataRef,
    span: std::ops::Range<f64>,
) -> Result<VolumeArchive, Stop> {
    let known: BTreeMap<Id, VolumeArchive> = member(&job.plan().await?, key)?;
    if let Some(archive) = known.get(&mount) {
        return Ok(archive.clone());
    }
    let part_bytes = {
        let mut conn = job
            .engine()
            .db()
            .acquire()
            .await
            .map_err(EngineError::from)?;
        crate::volume_carry::archive_part_bytes(&mut conn).await?
    };
    let value = job
        .command_part(
            Outgoing::new(
                crate::ids::derived(
                    "node.mount.export",
                    &[job.id().as_bytes(), mount.as_bytes(), key.as_bytes()],
                ),
                machine,
                data.link().key(),
                &NodeMountExport {
                    work: None,
                    driver: driver.into(),
                    job_id: job.id(),
                    mount_id: mount,
                    data: data.link(),
                    part_bytes,
                },
            )
            .map_err(EngineError::from)?,
            span,
        )
        .await?;
    let archive: VolumeArchive = serde_json::from_value(value).map_err(EngineError::from)?;
    let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
    let locked = jobs::lock(&mut tx, job.id())
        .await?
        .ok_or_else(|| copy_failed("the copy's job is gone"))?;
    let mut known: BTreeMap<Id, VolumeArchive> = member(&locked.plan, key)?;
    if let Some(archive) = known.get(&mount) {
        return Ok(archive.clone());
    }
    // Each part is held once for the job, however many of its archives share it: the release lets go of each once.
    let mut pins: Vec<Hash> = member(&locked.plan, done::PINS)?;
    let mut fresh: Vec<Hash> = archive
        .parts
        .iter()
        .map(|part| part.hash)
        .filter(|hash| !pins.contains(hash))
        .collect();
    fresh.sort();
    fresh.dedup();
    storage::pin_blobs(&mut tx, &fresh).await?;
    pins.extend(fresh);
    pins.sort();
    known.insert(mount, archive.clone());
    jobs::merge_plan(
        &mut tx,
        job.id(),
        &json!({ key: known, done::PINS: pins }),
        job.now(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(archive)
}

async fn unpack(
    job: &Job,
    word: &str,
    copy: &CopyDecision,
    volume: &DataRef,
    archive: &VolumeArchive,
    undoing: bool,
    span: Option<std::ops::Range<f64>>,
) -> Result<(), Stop> {
    let command = Outgoing::new(
        crate::ids::derived(
            "node.mount.import",
            &[
                job.id().as_bytes(),
                copy.mount_id.as_bytes(),
                word.as_bytes(),
            ],
        ),
        copy.target_machine,
        SerialKey::Volume(volume.source.clone()),
        &NodeMountImport {
            work: None,
            driver: copy.target_driver.clone(),
            job_id: job.id(),
            mount_id: copy.mount_id,
            name: volume.source.clone(),
            storage: volume.storage.clone(),
            archive: archive.clone(),
        },
    )
    .map_err(EngineError::from)?;
    if undoing {
        job.command_to_undo(command).await?;
    } else if let Some(span) = span {
        job.command_part(command, span).await?;
    } else {
        job.command(command).await?;
    }
    Ok(())
}

/// Carries out a confirmed plan's copies onto `machine` before the unit that uses them is made: what a replaced
/// location holds is copied aside first; then each mount's data is archived where it lies and unpacked where it
/// lands. A cancel stops it between commands, and the job's undo takes back what was done.
pub(crate) async fn apply(job: &Job, machine: Id) -> Result<(), Stop> {
    let decisions = of_plan(&job.plan().await?)?;
    let copies: Vec<&CopyDecision> = decisions
        .copies
        .iter()
        .filter(|copy| copy.target_machine == machine)
        .collect();
    let held_bytes = |copy: &CopyDecision| {
        copy.kept
            .as_ref()
            .filter(|kept| kept.copied_into.is_some())
            .and_then(|kept| kept.total_bytes)
            .unwrap_or(0)
    };
    let total = copies
        .iter()
        .fold(0i64, |sum, copy| {
            sum.saturating_add(copy.total_bytes)
                .saturating_add(held_bytes(copy))
        })
        .max(1);
    let mut carried = 0_i64;
    for copy in copies {
        let held_size = held_bytes(copy);
        let held_from = carried as f64 / total as f64;
        let held_mid = (carried as f64 + held_size as f64 / 2.0) / total as f64;
        let source_from = carried.saturating_add(held_size) as f64 / total as f64;
        let source_mid = (carried.saturating_add(held_size) as f64 + copy.total_bytes as f64 / 2.0)
            / total as f64;
        let source_to = carried
            .saturating_add(held_size)
            .saturating_add(copy.total_bytes) as f64
            / total as f64;
        let name = {
            let mut conn = job
                .engine()
                .db()
                .acquire()
                .await
                .map_err(EngineError::from)?;
            rows::mounts(&mut conn, &[copy.mount_id])
                .await?
                .pop()
                .map_or_else(|| copy.mount_id.to_string(), |row| row.name)
        };
        job.progress(
            carried as f64 / total as f64,
            Some(&format!(
                "copying the data of {name}: {} files, {} bytes",
                copy.file_count, copy.total_bytes
            )),
        )
        .await?;
        if copy.made {
            ensure(
                job,
                copy.target_machine,
                &copy.target_driver,
                copy.mount_id,
                &copy.to,
            )
            .await?;
        }
        if !copy.made {
            // What the location holds is archived before anything lands in it, even when that is nothing, so
            // that an undo can give it back exactly; a replace also keeps it in a volume of its own.
            let held = archived(
                job,
                done::KEPT_ARCHIVES,
                copy.mount_id,
                copy.target_machine,
                &copy.target_driver,
                &copy.to,
                held_from..held_mid,
            )
            .await?;
            if let Some(aside) = copy
                .kept
                .as_ref()
                .and_then(|kept| kept.copied_into.as_ref())
            {
                ensure(
                    job,
                    copy.target_machine,
                    &copy.target_driver,
                    copy.mount_id,
                    aside,
                )
                .await?;
                unpack(
                    job,
                    "aside",
                    copy,
                    aside,
                    &held,
                    false,
                    Some(held_mid..source_from),
                )
                .await?;
            }
        }
        let archive = archived(
            job,
            done::ARCHIVES,
            copy.mount_id,
            copy.source_machine,
            &copy.source_driver,
            &copy.from,
            source_from..source_mid,
        )
        .await?;
        if !copy.made {
            // From here on the location's own data is being replaced; an undo puts back what it held.
            let mut unpacking: Vec<Id> = member(&job.plan().await?, done::UNPACKING)?;
            if !unpacking.contains(&copy.mount_id) {
                unpacking.push(copy.mount_id);
                job.remember(&json!({ done::UNPACKING: unpacking })).await?;
            }
        }
        unpack(
            job,
            "copy",
            copy,
            &copy.to,
            &archive,
            false,
            Some(source_mid..source_to),
        )
        .await?;
        carried = carried
            .saturating_add(copy.total_bytes)
            .saturating_add(held_size);
        job.progress(carried as f64 / total as f64, None).await?;
    }
    Ok(())
}

/// Takes back a job's copies before anything became current: a location whose data a copy began to replace gets
/// back what it held, and every volume the job made is removed. What cannot be taken back is said in words.
pub(crate) async fn undo(job: &Job) -> Vec<String> {
    let mut left = Vec::new();
    let plan = match job.plan().await {
        Ok(plan) => plan,
        Err(error) => return vec![format!("the copies could not be read back: {error}")],
    };
    let (Ok(decisions), Ok(kept), Ok(unpacking)) = (
        of_plan(&plan),
        member::<BTreeMap<Id, VolumeArchive>>(&plan, done::KEPT_ARCHIVES),
        member::<Vec<Id>>(&plan, done::UNPACKING),
    ) else {
        return vec!["the copies could not be read back".to_owned()];
    };
    for copy in &decisions.copies {
        if !copy.made
            && unpacking.contains(&copy.mount_id)
            && let Some(held) = kept.get(&copy.mount_id)
            && unpack(job, "restore", copy, &copy.to, held, true, None)
                .await
                .is_err()
        {
            left.push(
                match copy
                    .kept
                    .as_ref()
                    .and_then(|kept| kept.copied_into.as_ref())
                {
                    Some(aside) => format!(
                        "the volume {} could not be given back what it held; that is kept in {}",
                        copy.to.source, aside.source
                    ),
                    None => format!(
                        "the volume {}, empty before the copy, could not be emptied again",
                        copy.to.source
                    ),
                },
            );
            continue;
        }
        let made = copy.made.then_some(&copy.to);
        let aside = copy
            .kept
            .as_ref()
            .and_then(|kept| kept.copied_into.as_ref());
        for volume in made.into_iter().chain(aside) {
            if remove(
                job,
                copy.target_machine,
                &copy.target_driver,
                copy.mount_id,
                volume,
            )
            .await
            .is_err()
            {
                left.push(format!(
                    "the volume {} the copy made could not be removed",
                    volume.source
                ));
            }
        }
    }
    left
}

/// Makes the copies' locations the machines' in the committing transaction, and lists what they displaced as kept,
/// attached nowhere until somebody purges it. Every other environment that attaches such a mount is told that its
/// location changed; `environment`, which the job made or moved with the copy attached, is not.
pub(crate) async fn commit(
    conn: &mut Conn,
    decisions: &MountDecisions,
    environment: Id,
    job: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    commit_locations(conn, decisions, Some(environment), job, by, at).await
}

/// Publishes location-only copies and tells every environment whose attachment changed.
pub(crate) async fn commit_locations(
    conn: &mut Conn,
    decisions: &MountDecisions,
    environment: Option<Id>,
    job: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<(), EngineError> {
    for copy in &decisions.copies {
        let current = rows::locations(conn, &[copy.mount_id])
            .await?
            .into_iter()
            .find(|row| row.machine_id == copy.target_machine);
        if copy.made
            && let Some(current) = current
            && (current.kind != copy.to.kind
                || current.source != copy.to.source
                || current.storage != copy.to.storage)
            && copy.kept.as_ref().is_none_or(|kept| {
                kept.previous.kind != current.kind
                    || kept.previous.source != current.source
                    || kept.previous.storage != current.storage
            })
        {
            // Concurrent accepted copies keep the data actually displaced, including a sibling's
            // newly published location. A stale preview must never forget that extra copy.
            rows::keep(
                conn,
                &rows::KeptRow {
                    id: new_id(),
                    mount_id: copy.mount_id,
                    machine_id: copy.target_machine,
                    kind: current.kind,
                    source: current.source,
                    storage: current.storage,
                    data_kind: current.data_kind,
                    file_count: None,
                    total_bytes: None,
                    job_id: Some(job),
                    kept_by: by,
                    kept_at: at,
                },
            )
            .await?;
        }
        rows::set_location(
            conn,
            &rows::LocationRow {
                mount_id: copy.mount_id,
                machine_id: copy.target_machine,
                host_id: None,
                host_name: None,
                kind: copy.to.kind.clone(),
                source: copy.to.source.clone(),
                storage: copy.to.storage.clone(),
                data_kind: copy.data_kind.clone(),
                created_by: by,
                created_at: at,
            },
        )
        .await?;
        rows::set_data_kind(
            conn,
            copy.mount_id,
            copy.target_machine,
            copy.data_kind.as_deref(),
        )
        .await?;
        if let Some(kept) = &copy.kept {
            let aside = kept.copied_into.as_ref().unwrap_or(&kept.previous);
            rows::keep(
                conn,
                &rows::KeptRow {
                    id: new_id(),
                    mount_id: copy.mount_id,
                    machine_id: copy.target_machine,
                    kind: aside.kind.clone(),
                    source: aside.source.clone(),
                    storage: aside.storage.clone(),
                    data_kind: kept.data_kind.clone(),
                    file_count: kept.file_count,
                    total_bytes: kept.total_bytes,
                    job_id: Some(job),
                    kept_by: by,
                    kept_at: at,
                },
            )
            .await?;
        }
        let mut others = rows::candidates(conn, copy.mount_id).await?;
        others.retain(|other| Some(*other) != environment);
        rows::changed(conn, &others, at).await?;
        let mut audit = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(by),
            orch_db::audit::name!("mount.location_added"),
            "mount",
            Some(copy.mount_id),
            "A copy of a mount's data became its location on a machine",
        );
        audit.details = json!({
            "mount": copy.mount_id, "machine": copy.target_machine, "kind": copy.to.kind,
            "job": job, "made": copy.made, "kept": copy.kept.is_some(),
        });
        orch_db::audit::append(conn, &audit, &orch_db::audit::Origin::default(), at).await?;
    }
    Ok(())
}

impl Engine {
    /// Lets go of the copies' archive parts once a job has ended, whatever its end.
    pub(crate) async fn after_mount_copies(&self, row: &jobs::JobRow) {
        if row.plan.get(done::PINS).is_none() {
            return;
        }
        let released = async {
            let mut tx = self.db().begin().await?;
            let Some(current) = jobs::lock(&mut tx, row.id).await? else {
                return Ok::<_, EngineError>(());
            };
            let pins: Vec<Hash> = current
                .plan
                .get(done::PINS)
                .cloned()
                .map(serde_json::from_value)
                .transpose()?
                .unwrap_or_default();
            if !pins.is_empty() {
                storage::unpin_blobs(&mut tx, &pins, self.inner.clock.now()).await?;
                jobs::merge_plan(
                    &mut tx,
                    row.id,
                    &json!({ done::PINS: [] }),
                    self.inner.clock.now(),
                )
                .await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok(())
        }
        .await;
        if let Err(error) = released {
            tracing::warn!(%error, "the archive parts of mount copies could not be released");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_made_by_a_copy_is_named_for_its_machine_and_for_the_copy_where_that_name_is_taken()
    {
        let mount = new_id();
        let machine = new_id();
        let seed = new_id();
        let first = made_name(mount, machine, seed, &[format!("orch-mount-{mount}")]);
        assert_eq!(first, format!("orch-mount-{mount}-{}", tail(machine)));
        assert!(orch_link_contract::commands::is_mount_volume(mount, &first));
        let second = made_name(mount, machine, seed, std::slice::from_ref(&first));
        assert_ne!(second, first);
        assert!(second.starts_with(&first));
        assert!(orch_link_contract::commands::is_mount_volume(
            mount, &second
        ));
        assert!(orch_link_contract::commands::is_mount_volume(
            mount,
            &kept_name(mount, machine, seed)
        ));
        assert!(
            !orch_link_contract::commands::is_mount_volume(new_id(), &first),
            "another mount's name is not its own"
        );
    }

    #[test]
    fn a_detached_mount_is_switched_off_in_the_environments_own_configuration_beside_the_rest() {
        let kept = new_id();
        let detached = new_id();
        let mut config = serde_json::json!({ "env.mounts": { kept.to_string(): true }, "env.storage": { "workspace": "fast" } });
        detach_in(&mut config, &[detached]);
        assert_eq!(config["env.mounts"][kept.to_string()], true);
        assert_eq!(config["env.mounts"][detached.to_string()], false);
        assert_eq!(config["env.storage"]["workspace"], "fast");
        let mut empty = serde_json::json!({});
        detach_in(&mut empty, &[]);
        assert_eq!(
            empty,
            serde_json::json!({}),
            "nothing detached changes nothing"
        );
    }

    #[test]
    fn copies_planned_on_a_machine_are_read_in_place_of_its_location() {
        let mount = new_id();
        let machine = new_id();
        let by = new_id();
        let decisions = MountDecisions {
            detached: Vec::new(),
            copies: vec![CopyDecision {
                mount_id: mount,
                source_machine: new_id(),
                source_driver: "docker".into(),
                from: DataRef::volume(format!("orch-mount-{mount}")),
                target_machine: machine,
                target_driver: "docker".into(),
                to: DataRef::volume(format!("orch-mount-{mount}-{}", tail(machine))),
                made: true,
                kept: None,
                file_count: 3,
                total_bytes: 30,
                data_kind: None,
            }],
        };
        let planned = decisions.planned(Some(machine), by, DateTime::<Utc>::UNIX_EPOCH);
        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].kind, "volume");
        assert!(planned[0].source.ends_with(&tail(machine)));
        assert!(
            decisions
                .planned(Some(new_id()), by, DateTime::<Utc>::UNIX_EPOCH)
                .is_empty()
        );
        assert_eq!(decisions.totals(), (3, 30));
    }
}
