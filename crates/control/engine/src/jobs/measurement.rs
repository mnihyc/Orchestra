//! Reusable background counts, retained with their observation time and cancelled when their last viewer leaves.

use async_trait::async_trait;
use chrono::Duration;
use orch_api_contract::forks::Measurement;
use orch_db::queries::jobs::{self as rows, NewJob};
use orch_link_contract::commands::{
    CommandKind, NodeMountFingerprint, NodeMountFingerprintResult, SerialKey, WorkReport,
};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time, new_id};
use serde_json::json;

use super::runner::{Done, Job, Phase, Phased, Stop, Undone, initial_progress};
use crate::commands::Outgoing;
use crate::{Engine, EngineError};

/// A count changes no data and takes no environment lifecycle lock.
pub const KIND: &str = "measurement";
/// Counting has no known total until the last entry is visited.
pub const PHASES: [Phase; 1] = [Phase {
    name: "measure",
    weight: 100,
}];

/// Two unknown-size reads have equal relative weights; each keeps its own counters and machine.
const FINGERPRINT_PHASES: [Phase; 2] = [
    Phase {
        name: "fingerprint_source",
        weight: 1,
    },
    Phase {
        name: "fingerprint_target",
        weight: 1,
    },
];

/// One explicit comparison, reused only by replans of that same request.
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct FingerprintInput {
    /// A fresh Check identity, distinct from an ordinary cached size count.
    pub request_id: Id,
    /// The source data; the job's machine is its source machine.
    pub source: NodeMountFingerprint,
    /// The destination machine to read after the source has stopped.
    pub target_machine: Id,
    /// The destination data.
    pub target: NodeMountFingerprint,
}

/// A measurement that a plan can show immediately, with the retained complete result when available.
pub(crate) struct Count {
    /// The retained identity and complete observation time, when available.
    pub reference: Measurement,
    /// The machine's complete answer, never a partial count.
    pub result: Option<Json>,
    /// A retained permanent refusal of an optional home copy, never an incomplete count.
    pub unsupported: bool,
}

impl Engine {
    /// Starts or reuses a count without waiting for its machine, including after an engine restart.
    pub(crate) async fn measure_in_background(
        &self,
        machine: Id,
        kind: CommandKind,
        body: Json,
        subject: Json,
        by: Id,
    ) -> Result<Count, EngineError> {
        self.measure_count(machine, kind, body, subject, by, false)
            .await
    }

    /// Optional move homes retain a permanent capability refusal for the ordinary count-valid window.
    pub(crate) async fn measure_home_in_background(
        &self,
        machine: Id,
        kind: CommandKind,
        body: Json,
        subject: Json,
        by: Id,
    ) -> Result<Count, EngineError> {
        self.measure_count(machine, kind, body, subject, by, true)
            .await
    }

    async fn measure_count(
        &self,
        machine: Id,
        kind: CommandKind,
        body: Json,
        subject: Json,
        by: Id,
        reuse_unsupported: bool,
    ) -> Result<Count, EngineError> {
        let now = self.inner.clock.now();
        let identity = orch_types::Hash::of(&serde_json::to_vec(&json!([
            machine,
            kind.as_str(),
            body,
            subject
        ]))?)
        .to_string();
        let mut tx = self.db().begin().await?;
        // Only requests for the same physical data serialize; no machine or filesystem wait holds this lock.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(&identity)
            .execute(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
        let keepalive = setting(&mut tx, keys::MEASUREMENT_KEEPALIVE_SECS).await?;
        let valid = setting(&mut tx, keys::MEASUREMENT_VALID_SECS).await?;
        let timeout = setting(&mut tx, keys::MEASUREMENT_TIMEOUT_SECS).await?;
        let existing: Option<Id> = sqlx::query_scalar(
            "SELECT id FROM jobs WHERE kind='measurement' AND plan->>'measurement_key'=$1
            AND ($3 OR (NOT (plan ? 'cancel') AND (state IN ('queued','running') OR
             ((state='succeeded' OR ($4 AND state='failed' AND error->>'code'='unsupported')) AND finished_at > $2)))) ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&identity)
        .bind(now - Duration::seconds(valid))
        .bind(kind == CommandKind::NodeMountFingerprint)
        .bind(reuse_unsupported)
        .fetch_optional(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
        let row = if let Some(id) = existing {
            rows::merge_plan(
                &mut tx,
                id,
                &json!({"keep_until": now + Duration::seconds(keepalive)}),
                now,
            )
            .await?;
            rows::job(&mut tx, id)
                .await?
                .ok_or_else(|| EngineError::unreadable("the count disappeared"))?
        } else {
            rows::insert(
                &mut tx,
                &NewJob {
                    id: new_id(),
                    kind: KIND,
                    node_id: None,
                    environment_id: None,
                    provider_id: None,
                    plan: &json!({"measurement_key": identity, "machine": machine,
                    "command_kind": kind.as_str(), "body": body, "subject": subject,
                    "keep_until": now + Duration::seconds(keepalive), "timeout_secs": timeout}),
                    progress: &serde_json::to_value(initial_progress(
                        if kind == CommandKind::NodeMountFingerprint {
                            &FINGERPRINT_PHASES
                        } else {
                            &PHASES
                        },
                    ))?,
                    created_by: Some(by),
                },
                now,
            )
            .await?
        };
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.kick();
        Ok(Count {
            reference: Measurement {
                job_id: row.id,
                measured_at: row
                    .finished_at
                    .filter(|_| row.state == "succeeded")
                    .map(Time::new),
            },
            unsupported: reuse_unsupported
                && row.state == "failed"
                && row.error.as_ref().and_then(|error| error["code"].as_str())
                    == Some("unsupported"),
            result: row.result,
        })
    }

    /// Marks unobserved counts for the same cooperative cancellation as an explicit Cancel.
    pub(crate) async fn expire_measurements(&self) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let ids: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM jobs WHERE kind='measurement'
            AND state IN ('queued','running') AND NOT (plan ? 'cancel') AND plan ? 'keep_until'",
        )
        .fetch_all(self.db().pool())
        .await
        .map_err(orch_db::DbError::from)?;
        for id in ids {
            let mut tx = self.db().begin().await?;
            let mut asked = false;
            if let Some(row) = rows::lock(&mut tx, id)
                .await?
                .filter(|row| row.is_open() && !row.cancel_requested())
            {
                if self.inner.live.job_is_watched(id) {
                    let seconds = setting(&mut tx, keys::MEASUREMENT_KEEPALIVE_SECS).await?;
                    rows::merge_plan(
                        &mut tx,
                        id,
                        &json!({"keep_until":now+Duration::seconds(seconds)}),
                        now,
                    )
                    .await?;
                } else if row.plan["keep_until"]
                    .as_str()
                    .and_then(|value| value.parse::<Time>().ok())
                    .is_some_and(|until| until.get() <= now)
                {
                    asked = rows::request_cancel_by(&mut tx, id, None, now).await?;
                }
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            if asked {
                self.stop_job_command(id).await?;
            }
        }
        Ok(())
    }
}

/// Resolves a job tunable from the system's settings registry.
pub(crate) async fn setting(conn: &mut orch_db::Conn, key: &str) -> Result<i64, EngineError> {
    orch_settings::get::<i64>(conn, orch_settings::Place::System, key)
        .await?
        .ok_or_else(|| EngineError::unreadable(format!("the job setting {key} is unavailable")))
}

/// A restartable count whose machine command has the same identity after a takeover.
pub(crate) struct Measuring;

#[async_trait]
impl Phased for Measuring {
    async fn run(&self, job: &Job, phase: &str) -> Result<Done, Stop> {
        let plan = job.plan().await?;
        let machine = plan["machine"]
            .as_str()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| EngineError::unreadable("the measurement has no machine"))?;
        let kind = plan["command_kind"]
            .as_str()
            .and_then(|value| value.parse::<CommandKind>().ok())
            .ok_or_else(|| EngineError::unreadable("the measurement has no command"))?;
        let timeout_secs = plan["timeout_secs"]
            .as_u64()
            .ok_or_else(|| EngineError::unreadable("the measurement has no time allowance"))?;
        if kind == CommandKind::NodeMountFingerprint {
            return fingerprint_phase(job, &plan, machine, phase, timeout_secs).await;
        }
        let mut body = plan["body"].clone();
        if kind != CommandKind::NodeResourcesList {
            body["work"] = serde_json::to_value(WorkReport {
                files_total: None,
                bytes_total: None,
                job_id: job.id(),
                phase: "measure".into(),
                timeout_secs,
            })
            .map_err(EngineError::from)?;
        }
        let result = job
            .command(Outgoing {
                id: crate::ids::derived("measurement", &[job.id().as_bytes()]),
                machine,
                kind,
                key: SerialKey::None,
                body,
                deadline: Some(Time::new(
                    job.now() + Duration::seconds(i64::try_from(timeout_secs).unwrap_or(i64::MAX)),
                )),
            })
            .await?;
        validate_count(kind, &plan["body"], &result)?;
        job.remember(&json!({"count":result})).await?;
        job.pass_commit().await?;
        Ok(Done::Finished)
    }
    async fn undo(&self, _job: &Job, _why: &Stop) -> Undone {
        Undone::Everything
    }
    async fn result(&self, job: &Job) -> Option<Json> {
        job.plan().await.ok()?.get("count").cloned()
    }
}

/// A comparison has one cancellation owner; the second read cannot be admitted after that owner is cancelled.
async fn fingerprint_phase(
    job: &Job,
    plan: &Json,
    source_machine: Id,
    phase: &str,
    timeout_secs: u64,
) -> Result<Done, Stop> {
    let input: FingerprintInput =
        serde_json::from_value(plan["body"].clone()).map_err(EngineError::from)?;
    let (machine, mut body) = match phase {
        "fingerprint_source" => (source_machine, input.source),
        "fingerprint_target" => (input.target_machine, input.target),
        _ => return Err(EngineError::unreadable("the comparison phase is invalid").into()),
    };
    body.work = Some(WorkReport {
        files_total: None,
        bytes_total: None,
        job_id: job.id(),
        phase: phase.into(),
        timeout_secs,
    });
    let row = job.row().await?;
    // Both reads share the comparison's original allowance, including after a worker takeover.
    let deadline = row.started_at.unwrap_or(row.created_at)
        + Duration::seconds(i64::try_from(timeout_secs).unwrap_or(i64::MAX));
    let result = job
        .command(Outgoing {
            id: crate::ids::derived(phase, &[job.id().as_bytes()]),
            machine,
            kind: CommandKind::NodeMountFingerprint,
            key: SerialKey::None,
            body: serde_json::to_value(body).map_err(EngineError::from)?,
            deadline: Some(Time::new(deadline)),
        })
        .await?;
    let read: NodeMountFingerprintResult =
        serde_json::from_value(result).map_err(EngineError::from)?;
    if phase == "fingerprint_source" {
        job.remember(&json!({"fingerprint_source":read.fingerprint}))
            .await?;
    } else {
        let source: orch_types::Hash = serde_json::from_value(plan["fingerprint_source"].clone())
            .map_err(EngineError::from)?;
        let result = orch_api_contract::forks::PlanMountFingerprints {
            checked_at: Time::new(job.now()),
            matches: source == read.fingerprint,
        };
        job.remember(&json!({"count":result})).await?;
        job.pass_commit().await?;
    }
    Ok(Done::Finished)
}

/// A malformed or incomplete answer fails the count instead of poisoning the reusable result.
fn validate_count(kind: CommandKind, body: &Json, result: &Json) -> Result<(), EngineError> {
    use orch_driver_contract::volumes::{OwnedVolumeSelector, VolumeMeasure};
    use orch_link_contract::commands::{
        NodeRuntimeVolumesMeasure, NodeRuntimeVolumesMeasureResult,
    };
    let valid = |measure: &VolumeMeasure| measure.file_count >= 0 && measure.total_bytes >= 0;
    match kind {
        CommandKind::NodeResourcesList => {
            let inventory: orch_driver_contract::resources::ResourceInventory =
                serde_json::from_value(result.clone())?;
            let unique = inventory
                .resources
                .iter()
                .map(|row| &row.resource)
                .collect::<std::collections::BTreeSet<_>>();
            if unique.len() != inventory.resources.len()
                || inventory
                    .resources
                    .iter()
                    .any(|row| row.size_bytes.is_some_and(|bytes| bytes < 0))
            {
                return Err(EngineError::unreadable(
                    "the owned-object inventory is invalid",
                ));
            }
        }
        CommandKind::NodeMountMeasure => {
            let measure: VolumeMeasure = serde_json::from_value(result.clone())?;
            if !valid(&measure) {
                return Err(EngineError::unreadable("the mount count is invalid"));
            }
        }
        CommandKind::NodeRuntimeVolumesMeasure => {
            let request: NodeRuntimeVolumesMeasure = serde_json::from_value(body.clone())?;
            let measured: NodeRuntimeVolumesMeasureResult = serde_json::from_value(result.clone())?;
            if measured.volumes.len() != request.volumes.len()
                || request.volumes.iter().any(|requested| {
                    let matches: Vec<_> = measured
                        .volumes
                        .iter()
                        .filter(|volume| volume.selector == requested.selector)
                        .collect();
                    matches.len() != 1
                        || matches.first().is_none_or(|volume| {
                            !valid(&volume.measure)
                                || requested.selector == OwnedVolumeSelector::FileStates
                                    && volume.measure.file_states.is_none_or(|store| {
                                        !store.complete
                                            || store.roots < 0
                                            || store.objects < 0
                                            || store.journals < 0
                                            || store.total_bytes < 0
                                    })
                        })
                })
            {
                return Err(EngineError::unreadable(
                    "the volume count is incomplete or invalid",
                ));
            }
        }
        CommandKind::FsDataMeasure => {
            let request: orch_link_contract::source::FsDataMeasure =
                serde_json::from_value(body.clone())?;
            let measured: orch_link_contract::source::FsDataMeasureResult =
                serde_json::from_value(result.clone())?;
            if measured.files < 0
                || measured.bytes < 0
                || request.part == orch_link_contract::source::EnvironmentDataPart::FileStates
                    && measured.recorded.is_none_or(|count| {
                        count.roots < 0 || count.objects < 0 || count.journals < 0
                    })
            {
                return Err(EngineError::unreadable(
                    "the data-root count is incomplete or invalid",
                ));
            }
        }
        _ => {
            return Err(EngineError::unreadable(
                "the command does not measure file data",
            ));
        }
    }
    Ok(())
}
