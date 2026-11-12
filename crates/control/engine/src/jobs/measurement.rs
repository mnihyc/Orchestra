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

pub mod preparation;
pub mod recording;
pub use preparation::{
    Observation as PreparedObservation, PendingReference, Preparation, ReferenceObservation,
    Request,
};
pub use recording::SavedMeasurements;

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
#[derive(Clone)]
pub struct Count {
    /// The retained identity and complete observation time, when available.
    pub reference: Measurement,
    /// The machine's complete answer, never a partial count.
    pub result: Option<Json>,
    /// A retained permanent refusal of an optional home copy, never an incomplete count.
    pub unsupported: bool,
    origin: Option<(orch_authz::CapturedOperation, Option<Time>)>,
}
impl Count {
    /// A known native reference remains unavailable when no completed data witness can be reused.
    pub(crate) fn unavailable(
        reference: Measurement,
        origin: &crate::operation_scope::http_origin::HttpOrigin,
    ) -> Self {
        Self {
            reference,
            result: None,
            unsupported: false,
            origin: Some((origin.origin().clone(), origin.until())),
        }
    }
    /// Exact accepted source minima for the caller's later native effect/reply preparation.
    /// These are retained restrictions, never reusable operation admission.
    pub fn source(&self) -> Option<(&orch_authz::CapturedOperation, Option<Time>)> {
        self.origin.as_ref().map(|(origin, until)| (origin, *until))
    }
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
        self.measure_request(
            Request {
                machine,
                kind,
                body,
                subject,
                by,
                reuse_unsupported: false,
            },
            None,
        )
        .await
    }
    /// The same supplier with a trusted native Source and immutable association.
    pub async fn measure_with_preparation(
        &self,
        request: Request,
        source: &Preparation,
    ) -> Result<Count, EngineError> {
        self.measure_observation(request.clone(), source)
            .await?
            .replay(&request, source)
    }
    /// Retains one native observation/pending reference for pure replay by the same planner.
    /// Replay carries data only; the consuming effect still obtains its current source frame.
    pub async fn measure_observation(
        &self,
        request: Request,
        source: &Preparation,
    ) -> Result<PreparedObservation, EngineError> {
        let identity = request.hash()?;
        let association = source.identity()?;
        let count = self.measure_request(request, Some(source)).await?;
        Ok(PreparedObservation::new(identity, association, count))
    }
    /// Existing fork planning consumes the same source-aware supplier; its native Source stays one definition.
    pub(crate) async fn measure_in_background_from(
        &self,
        machine: Id,
        kind: CommandKind,
        body: Json,
        subject: Json,
        by: Id,
        preparation: Option<&crate::fork_plan::operation::Preparation>,
    ) -> Result<Count, EngineError> {
        let request = Request {
            machine,
            kind,
            body,
            subject,
            by,
            reuse_unsupported: false,
        };
        match preparation.map(Preparation::from_fork).transpose()? {
            Some(source) => self.measure_with_preparation(request, &source).await,
            None => self.measure_request(request, None).await,
        }
    }
    pub(crate) async fn measure_home_in_background(
        &self,
        machine: Id,
        kind: CommandKind,
        body: Json,
        subject: Json,
        by: Id,
    ) -> Result<Count, EngineError> {
        self.measure_request(
            Request {
                machine,
                kind,
                body,
                subject,
                by,
                reuse_unsupported: true,
            },
            None,
        )
        .await
    }
    async fn measure_request(
        &self,
        request: Request,
        source: Option<&Preparation>,
    ) -> Result<Count, EngineError> {
        let count = if source.is_none()
            && let Some(planning) = crate::jobs::lifecycle_commands::planning::active()
        {
            planning.reuse_measurement(self, &request).await?
        } else {
            self.measure_request_from(request.clone(), source).await?
        };
        recording::observe(&request, &count)?;
        Ok(count)
    }
    async fn measure_request_from(
        &self,
        request: Request,
        source: Option<&Preparation>,
    ) -> Result<Count, EngineError> {
        let Request {
            machine,
            kind,
            ref body,
            ref subject,
            by,
            reuse_unsupported,
        } = request;
        let mut identity_value = json!([machine, kind.as_str(), body, subject]);
        if let Some(source) = source {
            identity_value
                .as_array_mut()
                .ok_or_else(|| EngineError::unreadable("invalid native measurement identity"))?
                .push(json!(["native_source_v1", source.identity()?]));
        }
        let identity = crate::jobs::command_identity::hash(&identity_value)?.to_string();
        let mut tx = self.db().begin().await?;
        let before = match source {
            Some(source) => Some(source.accept(self, &mut tx, &request, None).await?),
            None => None,
        };
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(&identity)
            .execute(&mut *tx)
            .await
            .map_err(orch_db::DbError::from)?;
        let mut accepted = match before {
            Some(before) => Some(before.refresh(self, &mut tx, &request, None).await?),
            None => None,
        };
        if accepted.as_ref().is_some_and(|source| source.actor() != by) {
            return Err(EngineError::unreadable(
                "the measurement actor differs from its original source",
            ));
        }
        let now = self.inner.clock.now();
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
        let mut reusable = None;
        if let Some(id) = existing {
            let row = rows::lock(&mut tx, id)
                .await?
                .ok_or_else(|| EngineError::unreadable("the count disappeared"))?;
            if let Some(current) = &accepted {
                accepted = Some(current.refresh(self, &mut tx, &request, Some(id)).await?);
            }
            if let Some(source) = source {
                let mut stored = preparation::read(&row)?.ok_or_else(|| {
                    EngineError::unreadable("the native count lost its original source")
                })?;
                if !stored.matches(&request, source)? {
                    return Err(EngineError::unreadable(
                        "the count belongs to another native source",
                    ));
                }
                if let Some(current) = &accepted {
                    stored.narrow(current)?;
                    preparation::save(&mut tx, &row, &stored, self.inner.clock.as_ref()).await?;
                }
                // An explicit comparison ID keeps its original two-phase owner even after failure
                // or cancellation. Only a new native request ID asks for another comparison.
                if kind == CommandKind::NodeMountFingerprint
                    || row.is_open() && !row.cancel_requested()
                    || stored.answered(&row)?
                {
                    reusable = rows::job(&mut tx, row.id).await?;
                }
            } else {
                reusable = Some(row);
            }
        }
        if let Some(current) = &accepted {
            current.check(self.inner.clock.as_ref())?;
        }
        let row = if let Some(row) = reusable {
            rows::merge_plan(
                &mut tx,
                row.id,
                &json!({"keep_until":now+Duration::seconds(keepalive)}),
                now,
            )
            .await?;
            row
        } else {
            let mut plan = json!({"measurement_key":identity,"machine":machine,"command_kind":kind.as_str(),"body":body,"subject":subject,
                "keep_until":now+Duration::seconds(keepalive),"timeout_secs":timeout,"measurement_optional_home":reuse_unsupported});
            if let Some(current) = &accepted
                && let Some(origin) = current.legacy_fork()
            {
                plan["fork_preparation"] = serde_json::to_value(origin)?;
            }
            let row = rows::insert(
                &mut tx,
                &NewJob {
                    id: new_id(),
                    kind: KIND,
                    node_id: None,
                    environment_id: None,
                    provider_id: None,
                    created_by: Some(by),
                    plan: &plan,
                    progress: &serde_json::to_value(initial_progress(
                        if kind == CommandKind::NodeMountFingerprint {
                            &FINGERPRINT_PHASES
                        } else {
                            &PHASES
                        },
                    ))?,
                },
                now,
            )
            .await?;
            if let (Some(current), Some(source)) = (&accepted, source) {
                let record = current
                    .record(self, &mut tx, &request, &row, source)
                    .await?;
                preparation::save(&mut tx, &row, &record, self.inner.clock.as_ref()).await?;
            }
            rows::job(&mut tx, row.id)
                .await?
                .ok_or_else(|| EngineError::unreadable("the count disappeared"))?
        };
        if let Some(current) = &accepted {
            current.check(self.inner.clock.as_ref())?;
        }
        let answered = if source.is_some() {
            preparation::read(&row)?
                .ok_or_else(|| EngineError::unreadable("the native count lost its source"))?
                .answered(&row)?
        } else {
            true
        };
        let tail = if source.is_some() {
            Some(
                preparation::read(&row)?
                    .ok_or_else(|| EngineError::unreadable("The native count lost its source."))?
                    .deadline(&mut tx, self.inner.clock.as_ref())
                    .await?,
            )
        } else {
            None
        };
        if let Some(tail) = &tail {
            tail.check(self.inner.clock.as_ref())?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if let Some(current) = &accepted {
            current.check(self.inner.clock.as_ref())?;
        }
        if let Some(tail) = &tail {
            tail.check(self.inner.clock.as_ref())?;
        }
        self.kick();
        let origin = if source.is_some() {
            preparation::read(&row)?
                .ok_or_else(|| EngineError::unreadable("The native count lost its source."))?
                .origin()
        } else {
            None
        };
        Ok(Count {
            origin,
            reference: Measurement {
                job_id: row.id,
                measured_at: row
                    .finished_at
                    .filter(|_| answered && row.state == "succeeded")
                    .map(Time::new),
            },
            unsupported: answered
                && reuse_unsupported
                && row.state == "failed"
                && row.error.as_ref().and_then(|error| error["code"].as_str())
                    == Some("unsupported"),
            result: if answered { row.result } else { None },
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
        let kind = plan["command_kind"]
            .as_str()
            .and_then(|value| value.parse::<CommandKind>().ok())
            .ok_or_else(|| EngineError::unreadable("the measurement has no command"))?;
        if kind == CommandKind::NodeMountFingerprint {
            return fingerprint_phase(job, &plan, phase).await;
        }
        let row = job.row().await?;
        let command =
            preparation::build_command(&row, preparation::MeasurementPhase::Measure, job.now())?;
        let command = planning_command(job, &plan, "measure", command).await?;
        let observed_command = command.clone();
        let result = measured_command(job, "measure", command).await?;
        validate_count(kind, &plan["body"], &result)?;
        preparation::observed(job, "measure", &observed_command, &result, false).await?;
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
async fn fingerprint_phase(job: &Job, plan: &Json, phase: &str) -> Result<Done, Stop> {
    let row = job.row().await?;
    let selected_phase = match phase {
        "fingerprint_source" => preparation::MeasurementPhase::FingerprintSource,
        "fingerprint_target" => preparation::MeasurementPhase::FingerprintTarget,
        _ => return Err(EngineError::unreadable("invalid comparison phase").into()),
    };
    let command = preparation::build_command(&row, selected_phase, job.now())?;
    let command = planning_command(job, plan, phase, command).await?;
    let observed_command = command.clone();
    let result = measured_command(job, phase, command).await?;
    preparation::observed(job, phase, &observed_command, &result, false).await?;
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

async fn planning_command(
    job: &Job,
    plan: &Json,
    phase: &str,
    command: Outgoing,
) -> Result<Outgoing, EngineError> {
    if preparation::is_retained(&job.row().await?) {
        return preparation::command(job, phase, command).await;
    }
    match plan.get("fork_preparation") {
        None => Ok(command),
        Some(value) => {
            let origin =
                serde_json::from_value::<crate::fork_plan::operation::Origin>(value.clone())?;
            crate::fork_plan::operation::Preparation::new(origin)
                .measurement_command(job.engine(), job.id(), phase, command)
                .await
        }
    }
}

async fn measured_command(job: &Job, phase: &str, command: Outgoing) -> Result<Json, Stop> {
    match job.command(command.clone()).await {
        Ok(result) => Ok(result),
        Err(stop) => {
            if let Some(status) = job
                .engine()
                .inner
                .commands
                .status(command.id)
                .await
                .map_err(EngineError::from)?
                && let Some(crate::commands::Outcome::Failed(error)) = status.outcome
                && error.code == "unsupported"
                && matches!(status.stage, crate::commands::Stage::Over)
            {
                preparation::observed(
                    job,
                    phase,
                    &command,
                    &serde_json::to_value(&error).map_err(EngineError::from)?,
                    true,
                )
                .await?;
            }
            Err(stop)
        }
    }
}
