//! Native measurement sources use the shared live preparation and private command channel.
use super::*;
use crate::{access::ResourceAccess, commands::CommandAuthority, harnesses::preparation as shared};
use orch_authz::{Caller, CapturedOperation};
use orch_db::{
    Conn,
    queries::{
        jobs::JobRow,
        link::{CommandRow, NewCommand},
    },
};
use orch_types::{Clock, Hash};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod observation;
mod provenance;
pub(crate) use observation::seal_finished;
use provenance::Origin;

pub use shared::MeasurementPhase;
pub(super) const FIELD: &str = "measurement_source";
const REQUIRED: &str = "measurement_source_version";
pub(super) fn is_retained(row: &JobRow) -> bool {
    row.plan.get(FIELD).is_some() || row.plan.get(REQUIRED).is_some()
}
fn closed() -> EngineError {
    EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused))
}

/// A domain's immutable association and actual live Source, never serialized runtime services.
#[derive(Clone)]
pub struct Preparation {
    association: Hash,
    source: Factory,
}
#[derive(Clone)]
enum Factory {
    Shared(shared::Preparation),
    Fork(Box<crate::fork_plan::operation::Preparation>),
    Inventory(Box<crate::jobs::lifecycle_commands::host_inventory::Preparation>),
}
impl Preparation {
    async fn check_observation_request(
        &self,
        engine: &Engine,
        conn: &mut Conn,
        row: &JobRow,
    ) -> Result<(), EngineError> {
        if let Factory::Fork(source) = &self.source {
            let check = source.accept_on(engine, conn).await?;
            let kind = row.plan["command_kind"]
                .as_str()
                .and_then(|value| value.parse().ok())
                .ok_or_else(closed)?;
            for phase in phases_for(kind) {
                let command = build_command(row, phase, row.started_at.unwrap_or(row.created_at))?;
                if !check
                    .origin
                    .input()
                    .supplier(
                        conn,
                        command.machine,
                        command.kind,
                        &command.body,
                        engine.inner.clock.now(),
                    )
                    .await?
                {
                    return Err(closed());
                }
            }
            check.check(engine.inner.clock.as_ref())?;
        }
        Ok(())
    }

    /// The association hashes the domain's actual native source/request, including its raw identity.
    /// It is only a cache identity; the supplied Source still proves every current operand.
    pub fn new(association: Hash, source: shared::Preparation) -> Self {
        Self {
            association,
            source: Factory::Shared(source),
        }
    }
    pub fn from_fork(
        source: &crate::fork_plan::operation::Preparation,
    ) -> Result<Self, EngineError> {
        Ok(Self {
            association: crate::jobs::command_identity::hash(&serde_json::to_value(
                source.origin(),
            )?)?,
            source: Factory::Fork(Box::new(source.clone())),
        })
    }
    pub fn from_host_inventory(
        source: crate::jobs::lifecycle_commands::host_inventory::Preparation,
    ) -> Result<Self, EngineError> {
        Ok(Self {
            association: source.association()?,
            source: Factory::Inventory(Box::new(source)),
        })
    }
    fn original(&self) -> Origin {
        match &self.source {
            Factory::Shared(source) => Origin::Operation(source.origin().clone()),
            Factory::Fork(source) => Origin::Operation(source.origin().captured().clone()),
            Factory::Inventory(source) => Origin::Inventory(source.original().clone()),
        }
    }
    pub(super) fn identity(&self) -> Result<Hash, EngineError> {
        crate::jobs::command_identity::hash(&json!([self.association, self.original()]))
    }
    pub(super) async fn accept(
        &self,
        engine: &Engine,
        conn: &mut Conn,
        request: &Request,
        job: Option<Id>,
    ) -> Result<Accepted, EngineError> {
        let physical = request.hash()?;
        match &self.source {
            Factory::Fork(source) => {
                let check = source.accept_on(engine, conn).await?;
                check.check(engine.inner.clock.as_ref())?;
                Ok(Accepted {
                    preparation: Self {
                        association: self.association,
                        source: Factory::Fork(source.clone()),
                    },
                    origin: Origin::Operation(check.origin.captured().clone()),
                    until: check.until(),
                    caller: check.caller().clone(),
                })
            }
            Factory::Shared(source) => {
                let target = shared::Target::Measurement {
                    job,
                    machine: request.machine,
                    kind: request.kind,
                    identity: physical,
                    phase: None,
                };
                let check = source
                    .checkpoint(conn, engine.inner.clock.as_ref(), target)
                    .await?;
                let accepted = source.accepted_at(&check)?;
                let result = Accepted {
                    origin: Origin::Operation(accepted.origin().clone()),
                    until: accepted.until(),
                    caller: check.caller().clone(),
                    preparation: Self {
                        association: self.association,
                        source: Factory::Shared(accepted),
                    },
                };
                check.check(engine.inner.clock.as_ref())?;
                Ok(result)
            }
            Factory::Inventory(source) => {
                if !source.matches_request(request) {
                    return Err(closed());
                }
                let (check, origin) = source.checkpoint(conn, engine.inner.clock.as_ref()).await?;
                let preparation = source.accepted(origin.clone())?;
                let caller = check.caller().clone();
                let until = check.until();
                check.check(engine.inner.clock.as_ref())?;
                Ok(Accepted {
                    preparation: Self {
                        association: self.association,
                        source: Factory::Inventory(Box::new(preparation)),
                    },
                    origin: Origin::Inventory(origin),
                    until,
                    caller,
                })
            }
        }
    }
}
pub(super) struct Accepted {
    preparation: Preparation,
    origin: Origin,
    until: Option<Time>,
    caller: Caller,
}
impl Accepted {
    pub(super) fn actor(&self) -> Id {
        self.origin.user()
    }
    pub(super) async fn refresh(
        &self,
        engine: &Engine,
        conn: &mut Conn,
        request: &Request,
        job: Option<Id>,
    ) -> Result<Self, EngineError> {
        self.preparation.accept(engine, conn, request, job).await
    }
    pub(super) fn check(&self, clock: &dyn Clock) -> Result<(), EngineError> {
        if !self.caller.key_deadline_allows(clock)
            || self.until.is_some_and(|until| clock.time() >= until)
        {
            return Err(closed());
        }
        Ok(())
    }
    pub(super) fn legacy_fork(&self) -> Option<&crate::fork_plan::operation::Origin> {
        match &self.preparation.source {
            Factory::Fork(source) => Some(source.origin()),
            _ => None,
        }
    }
    pub(super) async fn record(
        &self,
        engine: &Engine,
        conn: &mut Conn,
        request: &Request,
        row: &JobRow,
        original: &Preparation,
    ) -> Result<Stored, EngineError> {
        if self.origin.user() != request.by {
            return Err(closed());
        }
        if let Factory::Inventory(source) = &self.preparation.source {
            let command = build_command(row, MeasurementPhase::Measure, engine.inner.clock.now())?;
            let (authority, origin) = source
                .command_on(conn, engine.inner.clock.as_ref(), row, request, &command)
                .await?;
            return Ok(Stored {
                version: 1,
                association: self.preparation.association,
                original: original.identity()?,
                request: request.hash()?,
                origin: Origin::Inventory(origin),
                until: self.until,
                seeds: [(MeasurementPhase::Measure, authority)]
                    .into_iter()
                    .collect(),
                observations: BTreeMap::new(),
            });
        }
        let mut seeds = BTreeMap::new();
        let mut origin = self.origin.clone();
        let mut until = self.until;
        for phase in phases_for(request.kind) {
            // This is an unsent native preparation. The worker will seal its real start-time budget.
            let command = build_command(row, phase, engine.inner.clock.now())?;
            let authority = match &self.preparation.source {
                Factory::Fork(source) => {
                    source
                        .measurement_command_on(
                            conn,
                            engine.inner.clock.as_ref(),
                            engine.inner.access.as_ref(),
                            row.id,
                            phase.as_str(),
                            command,
                        )
                        .await?
                }
                Factory::Inventory(_) => return Err(closed()),
                Factory::Shared(source) => {
                    source
                        .command_authority_on(
                            conn,
                            engine.inner.clock.as_ref(),
                            shared::Target::Measurement {
                                job: Some(row.id),
                                machine: command.machine,
                                kind: command.kind,
                                identity: request.hash()?,
                                phase: Some(phase),
                            },
                            &command,
                        )
                        .await?
                }
            };
            let outgoing = build_command(row, phase, engine.inner.clock.now())?;
            let (authority, next, bound) = authority
                .reprepare_measurement(
                    conn,
                    engine.inner.clock.as_ref(),
                    engine.inner.access.as_ref(),
                    origin.complete().ok_or_else(closed)?,
                    until,
                    &outgoing,
                )
                .await?;
            if next.user() != self.origin.user() || next.operation() != self.origin.operation() {
                return Err(closed());
            }
            origin = Origin::Operation(next);
            until = until.into_iter().chain(bound).min();
            seeds.insert(phase, authority);
        }
        self.check(engine.inner.clock.as_ref())?;
        Ok(Stored {
            version: 1,
            association: self.preparation.association,
            original: original.identity()?,
            request: request.hash()?,
            origin,
            until,
            seeds,
            observations: BTreeMap::new(),
        })
    }
}

/// Consume only an actual domain verifier's fresh native proof; metadata alone never admits work.
pub(crate) async fn retain_current_callers(
    conn: &mut Conn,
    clock: &dyn Clock,
    origin: &CapturedOperation,
    until: Option<Time>,
    callers: &[(Option<Caller>, Option<Time>)],
) -> Result<(CapturedOperation, Option<Time>), EngineError> {
    let mut restored = origin
        .restore_current(conn, clock)
        .await?
        .ok_or_else(closed)?;
    let mut result = None;
    let mut end = until;
    for (caller, bound) in callers {
        end = end.into_iter().chain(*bound).min();
        if let Some(proof) = caller {
            if !proof.key_deadline_allows(clock) {
                return Err(closed());
            }
            if proof.user() == origin.user()
                && proof.admitted_operation() == Some(origin.operation())
            {
                let caller = restored
                    .narrow_current_admission(proof)
                    .map_err(|_| closed())?;
                let plan = crate::operation_scope::Prepared::for_selection(origin.operation())
                    .map_err(EngineError::unreadable)?;
                if !crate::operation_scope::compatible_selection(&caller, plan.variant()) {
                    return Err(closed());
                }
                result = Some(
                    origin
                        .narrowed_with(&caller)
                        .map_err(EngineError::unreadable)?,
                );
                restored = caller;
            }
        }
    }
    if end.is_some_and(|end| clock.time() >= end) {
        return Err(closed());
    }
    Ok((result.ok_or_else(closed)?, end))
}

/// One actual saved native reference, under the new consumer's current source.
/// Neither state adopts the old requester or authorizes forward IO.
#[derive(Clone)]
pub enum ReferenceObservation {
    /// Actual matching queued/running native claim; no result has been proven.
    Pending(PendingReference),
    /// Actual terminal native answer/witness, including optional Unsupported.
    Complete(Observation),
}
/// A genuine pending reference can only be constructed by the native data reader.
#[derive(Clone)]
pub struct PendingReference(Observation);
impl PendingReference {
    /// Data-only replay with the actual new Request/Source. The returned Count
    /// has the saved reference, no result/measurement time, and current source minima.
    pub fn replay(&self, request: &Request, source: &Preparation) -> Result<Count, EngineError> {
        self.0.replay(request, source)
    }
}
impl ReferenceObservation {
    /// The actual saved job/reference; never replaced by a new claim.
    pub fn reference(&self) -> &orch_api_contract::forks::Measurement {
        match self {
            Self::Pending(pending) => &pending.0.count.reference,
            Self::Complete(observation) => &observation.count.reference,
        }
    }
    /// Data-only replay. A pending Count cannot be confused with a measured zero.
    pub fn replay(&self, request: &Request, source: &Preparation) -> Result<Count, EngineError> {
        match self {
            Self::Pending(pending) => pending.replay(request, source),
            Self::Complete(observation) => observation.replay(request, source),
        }
    }
    pub(super) fn completed(self) -> Option<Observation> {
        match self {
            Self::Complete(observation) => Some(observation),
            Self::Pending(_) => None,
        }
    }
}

/// The existing native request, with its real optional-home behavior.
#[derive(Clone)]
pub struct Request {
    pub machine: Id,
    pub kind: CommandKind,
    pub body: Json,
    pub subject: Json,
    pub by: Id,
    pub reuse_unsupported: bool,
}
impl Request {
    fn physical(&self) -> Result<Hash, EngineError> {
        crate::jobs::command_identity::hash(&json!([
            self.machine,
            self.kind.as_str(),
            self.body,
            self.subject
        ]))
    }
    pub(super) fn hash(&self) -> Result<Hash, EngineError> {
        crate::jobs::command_identity::hash(&json!([
            self.machine,
            self.kind.as_str(),
            self.body,
            self.subject,
            self.by,
            self.reuse_unsupported
        ]))
    }
}
/// A genuine supplier answer retained for effect-free replay of the identical native request.
/// It is data, never admission, and cannot be constructed from a terminal job marker.
#[derive(Clone)]
pub struct Observation {
    request: Hash,
    association: Hash,
    count: Count,
    witness: Option<Hash>,
}
impl Observation {
    pub(super) fn new(request: Hash, association: Hash, count: Count) -> Self {
        Self {
            request,
            association,
            count,
            witness: None,
        }
    }
    /// The caller's current native source/effect guard remains mandatory when these facts are used.
    pub fn replay(&self, request: &Request, source: &Preparation) -> Result<Count, EngineError> {
        if self.request != request.hash()? || self.association != source.identity()? {
            return Err(closed());
        }
        Ok(self.count.clone())
    }
    /// Immutable canonical native observation witness; never permission evidence.
    pub fn witness(&self) -> Option<Hash> {
        self.witness
    }
}
fn row_request_hash(row: &JobRow) -> Result<Hash, EngineError> {
    crate::jobs::command_identity::hash(&json!([
        row.plan["machine"],
        row.plan["command_kind"],
        row.plan["body"],
        row.plan["subject"],
        row.created_by,
        row.plan["measurement_optional_home"] == json!(true)
    ]))
}
fn row_physical(row: &JobRow) -> Result<Hash, EngineError> {
    crate::jobs::command_identity::hash(&json!([
        row.plan["machine"],
        row.plan["command_kind"],
        row.plan["body"],
        row.plan["subject"]
    ]))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Stored {
    version: u8,
    association: Hash,
    original: Hash,
    request: Hash,
    origin: Origin,
    until: Option<Time>,
    seeds: BTreeMap<MeasurementPhase, CommandAuthority>,
    observations: BTreeMap<MeasurementPhase, Answer>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    command: Id,
    machine: Id,
    body: Hash,
    result: Hash,
    fingerprint: Option<Hash>,
    unsupported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    frame: Option<Frame>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    base: Id,
    canonical: Hash,
}
pub(super) fn read(row: &JobRow) -> Result<Option<Stored>, EngineError> {
    match (row.plan.get(FIELD), row.plan.get(REQUIRED)) {
        (None, None) => Ok(None),
        (Some(value), Some(version)) if version == &json!(1) => {
            let stored: Stored = serde_json::from_value(value.clone()).map_err(|_| closed())?;
            if stored.version != 1
                || stored.request != row_request_hash(row)?
                || row.created_by != Some(stored.origin.user())
                || stored.seeds.keys().copied().collect::<Vec<_>>()
                    != phases_for(
                        row.plan["command_kind"]
                            .as_str()
                            .and_then(|s| s.parse().ok())
                            .ok_or_else(closed)?,
                    )
            {
                return Err(closed());
            }
            Ok(Some(stored))
        }
        _ => Err(closed()),
    }
}
pub(super) async fn save(
    conn: &mut Conn,
    row: &JobRow,
    stored: &Stored,
    clock: &dyn Clock,
) -> Result<(), EngineError> {
    rows::merge_plan(conn, row.id, &json!({FIELD:stored,REQUIRED:1}), clock.now()).await?;
    Ok(())
}
impl Stored {
    pub(super) async fn deadline(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<shared::Deadline, EngineError> {
        let caller = self
            .origin
            .restore_current(conn, clock)
            .await?
            .ok_or_else(closed)?;
        let deadline =
            shared::Checkpoint::from_check(caller, self.until, Box::new(|_| Ok(()))).deadline();
        deadline.check(clock)?;
        Ok(deadline)
    }
    pub(super) fn matches(
        &self,
        request: &Request,
        source: &Preparation,
    ) -> Result<bool, EngineError> {
        Ok(self.request == request.hash()?
            && self.association == source.association
            && self.original == source.identity()?)
    }
    pub(super) fn origin(&self) -> Option<(CapturedOperation, Option<Time>)> {
        self.origin
            .complete()
            .cloned()
            .map(|origin| (origin, self.until))
    }
    pub(super) fn narrow(&mut self, accepted: &Accepted) -> Result<(), EngineError> {
        self.origin = self
            .origin
            .narrowed_with(&accepted.caller, accepted.until)?;
        self.until = self.until.into_iter().chain(accepted.until).min();
        Ok(())
    }
    pub(super) fn answered(&self, row: &JobRow) -> Result<bool, EngineError> {
        if let Some(answered) = observation::terminal_witness(row)? {
            return Ok(answered);
        }
        answered(row, &self.observations)
    }
}
fn answered(
    row: &JobRow,
    observations: &BTreeMap<MeasurementPhase, Answer>,
) -> Result<bool, EngineError> {
    let phases = phases_for(
        row.plan["command_kind"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .ok_or_else(closed)?,
    );
    for phase in phases {
        let Some(receipt) = observations.get(&phase) else {
            return Ok(false);
        };
        let command = build_command(row, phase, row.started_at.unwrap_or(row.created_at))?;
        if receipt
            .frame
            .as_ref()
            .map_or(receipt.command, |frame| frame.base)
            != command.id
            || receipt.machine != command.machine
            || receipt.body != crate::jobs::command_identity::hash(&command.body)?
        {
            return Ok(false);
        }
        if let Some(frame) = &receipt.frame {
            let canonical = crate::jobs::command_identity::canonical(
                receipt.command,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?;
            if crate::jobs::command_identity::hash(&canonical)? != frame.canonical {
                return Ok(false);
            }
        }
    }
    if row.state == "succeeded" {
        let Some(result) = row
            .result
            .as_ref()
            .filter(|result| row.plan.get("count") == Some(*result))
        else {
            return Ok(false);
        };
        if let Some(observation) = observations.get(&MeasurementPhase::Measure) {
            Ok(!observation.unsupported
                && observation.result == crate::jobs::command_identity::hash(result)?)
        } else {
            let source = observations
                .get(&MeasurementPhase::FingerprintSource)
                .and_then(|o| o.fingerprint);
            let target = observations
                .get(&MeasurementPhase::FingerprintTarget)
                .and_then(|o| o.fingerprint);
            Ok(source.is_some()
                && target.is_some()
                && row.plan.get("fingerprint_source") == Some(&serde_json::to_value(source)?)
                && result.get("matches") == Some(&json!(source == target))
                && observations
                    .values()
                    .all(|observation| !observation.unsupported))
        }
    } else {
        Ok(row.state == "failed"
            && row.plan["measurement_optional_home"] == json!(true)
            && observations
                .get(&MeasurementPhase::Measure)
                .is_some_and(|r| r.unsupported))
    }
}
pub(crate) fn inventory_origin(
    row: &JobRow,
) -> Result<Option<orch_authz::http_origin::HttpPreparation>, EngineError> {
    Ok(read(row)?.and_then(|stored| stored.origin.prefix().cloned()))
}
pub(crate) fn request_of(row: &JobRow) -> Result<Request, EngineError> {
    Ok(Request {
        machine: serde_json::from_value(row.plan["machine"].clone())?,
        kind: row.plan["command_kind"]
            .as_str()
            .ok_or_else(closed)?
            .parse()
            .map_err(|_| closed())?,
        body: row.plan["body"].clone(),
        subject: row.plan["subject"].clone(),
        by: row.created_by.ok_or_else(closed)?,
        reuse_unsupported: row.plan["measurement_optional_home"]
            .as_bool()
            .ok_or_else(closed)?,
    })
}
pub(crate) fn inventory_deadline(row: &JobRow) -> Result<Time, EngineError> {
    let secs = row.plan["timeout_secs"]
        .as_u64()
        .and_then(|secs| i64::try_from(secs).ok())
        .ok_or_else(closed)?;
    Ok(Time::new(
        row.started_at.ok_or_else(closed)? + Duration::seconds(secs),
    ))
}

pub(super) fn phases_for(kind: CommandKind) -> Vec<MeasurementPhase> {
    if kind == CommandKind::NodeMountFingerprint {
        vec![
            MeasurementPhase::FingerprintSource,
            MeasurementPhase::FingerprintTarget,
        ]
    } else {
        vec![MeasurementPhase::Measure]
    }
}

/// One native composer supplies claim templates, workers and issue/send association checks.
pub(crate) fn build_command(
    row: &JobRow,
    phase: MeasurementPhase,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Outgoing, EngineError> {
    let kind: CommandKind = row.plan["command_kind"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(closed)?;
    if !matches!(
        kind,
        CommandKind::NodeResourcesList
            | CommandKind::NodeMountMeasure
            | CommandKind::NodeMountFingerprint
            | CommandKind::NodeRuntimeVolumesMeasure
            | CommandKind::FsDataMeasure
    ) {
        return Err(closed());
    }
    let source_machine: Id = serde_json::from_value(row.plan["machine"].clone())?;
    let timeout = row.plan["timeout_secs"].as_u64().ok_or_else(closed)?;
    let work = WorkReport {
        files_total: None,
        bytes_total: None,
        job_id: row.id,
        phase: phase.as_str().to_owned(),
        timeout_secs: timeout,
    };
    let (machine, body, id, deadline) = if kind == CommandKind::NodeMountFingerprint {
        let input: FingerprintInput = serde_json::from_value(row.plan["body"].clone())?;
        let (machine, mut body) = match phase {
            MeasurementPhase::FingerprintSource => (source_machine, input.source),
            MeasurementPhase::FingerprintTarget => (input.target_machine, input.target),
            _ => return Err(closed()),
        };
        body.work = Some(work);
        (
            machine,
            serde_json::to_value(body)?,
            crate::ids::derived(phase.as_str(), &[row.id.as_bytes()]),
            row.started_at.unwrap_or(row.created_at)
                + Duration::seconds(i64::try_from(timeout).unwrap_or(i64::MAX)),
        )
    } else {
        if phase != MeasurementPhase::Measure {
            return Err(closed());
        }
        let mut body = row.plan["body"].clone();
        if kind != CommandKind::NodeResourcesList {
            body["work"] = serde_json::to_value(work)?;
        }
        (
            source_machine,
            body,
            crate::ids::derived("measurement", &[row.id.as_bytes()]),
            now + Duration::seconds(i64::try_from(timeout).unwrap_or(i64::MAX)),
        )
    };
    Ok(Outgoing {
        authority: None,
        id,
        machine,
        kind,
        key: SerialKey::None,
        body,
        deadline: Some(Time::new(deadline)),
    })
}
fn phase_of(phase: &str) -> Result<MeasurementPhase, EngineError> {
    match phase {
        "measure" => Ok(MeasurementPhase::Measure),
        "fingerprint_source" => Ok(MeasurementPhase::FingerprintSource),
        "fingerprint_target" => Ok(MeasurementPhase::FingerprintTarget),
        _ => Err(closed()),
    }
}

/// Private source evidence for the actual native measurement frame; never machine wire data.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEvidence {
    job: Id,
    phase: MeasurementPhase,
    request: Hash,
    inner: Box<CommandAuthority>,
}
impl std::fmt::Debug for CommandEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MeasurementCommand([private])")
    }
}
impl CommandEvidence {
    pub(crate) fn command_kind(&self) -> &'static str {
        self.inner.command_kind()
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.inner.lock_native_source(conn).await?;
        let _ = rows::lock(conn, self.job).await?;
        Ok(())
    }
    async fn checked(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
        command: &Outgoing,
    ) -> Result<(CommandAuthority, Origin, Option<Time>), EngineError> {
        let row = rows::lock(conn, self.job).await?.ok_or_else(closed)?;
        let mut stored = read(&row)?.ok_or_else(closed)?;
        if row.kind != KIND
            || !row.is_open()
            || row.cancel_requested()
            || super::super::runner::abort_reason(&row)?.is_some()
            || stored.request != self.request
        {
            return Err(closed());
        }
        let progress = super::super::runner::read_progress(&row).0;
        if progress.phase != self.phase.as_str() {
            return Err(closed());
        }
        let expected = build_command(&row, self.phase, clock.now())?;
        if command.id != expected.id
            || command.machine != expected.machine
            || command.kind != expected.kind
            || command.key != expected.key
            || command.body != expected.body
            || command.deadline.is_none()
            || command.deadline > expected.deadline
        {
            return Err(closed());
        }
        if command.kind == CommandKind::NodeMountFingerprint
            && command.deadline != expected.deadline
        {
            return Err(closed());
        }
        let (inner, origin, until) = if let Some(original) = stored.origin.complete() {
            let (inner, origin, until) = self
                .inner
                .reprepare_measurement(conn, clock, access, original, stored.until, command)
                .await?;
            (inner, Origin::Operation(origin), until)
        } else {
            let serial = command.key.wire();
            let proposed = NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: serial.as_deref(),
                deadline_at: command.deadline.ok_or_else(closed)?.get(),
            };
            let callers = Box::pin(
                self.inner
                    .current_issue_callers(conn, &proposed, clock, access),
            )
            .await?
            .ok_or_else(closed)?;
            let (origin, until) = stored
                .origin
                .retain(conn, clock, stored.until, &callers)
                .await?;
            ((*self.inner).clone(), origin, until)
        };
        if origin.user() != stored.origin.user() || origin.operation() != stored.origin.operation()
        {
            return Err(closed());
        }
        stored.origin = origin.clone();
        stored.until = stored.until.into_iter().chain(until).min();
        stored.seeds.insert(self.phase, inner.clone());
        save(conn, &row, &stored, clock).await?;
        Ok((inner, origin, stored.until))
    }
    async fn retained_callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        until: Option<Time>,
        mut callers: Vec<(Option<Caller>, Option<Time>)>,
    ) -> Result<Vec<(Option<Caller>, Option<Time>)>, EngineError> {
        let row = rows::job(conn, self.job).await?.ok_or_else(closed)?;
        let record = read(&row)?.ok_or_else(closed)?;
        let restored = record
            .origin
            .restore_current(conn, clock)
            .await?
            .ok_or_else(closed)?;
        for (caller, bound) in &mut callers {
            *bound = (*bound).into_iter().chain(until).min();
            if let Some(proof) = caller {
                if record.origin.admits(proof) {
                    *proof = record.origin.narrow_caller(&restored, proof)?;
                } else if record.origin.prefix().is_some() {
                    return Err(closed());
                }
            }
        }
        Ok(callers)
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let actual = Outgoing {
            id: command.id,
            machine: command.machine_id,
            kind: command.kind.parse().map_err(|_| closed())?,
            key: SerialKey::None,
            body: command.body.clone(),
            deadline: Some(Time::new(command.deadline_at)),
            authority: None,
        };
        if command.serial_key.is_some() {
            return Ok(None);
        }
        match self.checked(conn, clock, access, &actual).await {
            Ok((inner, _, until)) => {
                let result =
                    Box::pin(inner.current_issue_callers(conn, command, clock, access)).await?;
                let Some(callers) = result else {
                    return Ok(None);
                };
                self.retained_callers(conn, clock, until, callers)
                    .await
                    .map(Some)
            }
            Err(EngineError::BeforeEffect(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
        access: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let actual = Outgoing {
            id: command.id,
            machine: command.machine_id,
            kind: command.kind.parse().map_err(|_| closed())?,
            key: SerialKey::None,
            body: command.body.clone(),
            deadline: Some(Time::new(command.deadline_at)),
            authority: None,
        };
        if command.serial_key().is_some() {
            return Ok(None);
        }
        match self.checked(conn, clock, access, &actual).await {
            Ok((inner, _, until)) => {
                let result = Box::pin(inner.current_callers(conn, command, clock, access)).await?;
                let Some(callers) = result else {
                    return Ok(None);
                };
                self.retained_callers(conn, clock, until, callers)
                    .await
                    .map(Some)
            }
            Err(EngineError::BeforeEffect(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }
}

pub(super) async fn command(
    job: &Job,
    phase: &str,
    mut command: Outgoing,
) -> Result<Outgoing, EngineError> {
    let engine = job.engine();
    // A real accepted/terminal command keeps its native answer/reconciliation path.
    // Unaccepted work below still rechecks the original source before every new effect.
    let canonical = {
        let mut conn = engine.db().acquire().await?;
        orch_db::queries::link::command(&mut conn, command.id).await?
    };
    if let Some(canonical) = canonical {
        command.deadline = Some(Time::new(canonical.deadline_at));
        if !matches!(canonical.state.as_str(), "queued" | "sent") {
            return crate::harnesses::preparation::authorize_command(engine, command).await;
        }
    }
    let snapshot = job.row().await?;
    let Some(record) = read(&snapshot)? else {
        return Ok(command);
    };
    if record.origin.prefix().is_some() {
        let end = inventory_deadline(&snapshot)?;
        command.deadline = Some(command.deadline.map_or(end, |until| until.min(end)));
    }
    let phase = phase_of(phase)?;
    let seed = record.seeds.get(&phase).ok_or_else(closed)?;
    let evidence = CommandEvidence {
        job: job.id(),
        phase,
        request: record.request,
        inner: Box::new(seed.clone()),
    };
    let mut tx = engine.db().begin().await?;
    evidence.lock_source(&mut tx).await?;
    let (inner, _, until) = evidence
        .checked(
            &mut tx,
            engine.inner.clock.as_ref(),
            engine.inner.access.as_ref(),
            &command,
        )
        .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if until.is_some_and(|until| engine.inner.clock.time() >= until) {
        return Err(closed());
    }
    command.authority = Some(CommandAuthority::Measurement(Box::new(CommandEvidence {
        inner: Box::new(inner),
        ..evidence
    })));
    crate::harnesses::preparation::authorize_command(engine, command).await
}

pub(super) async fn observed(
    job: &Job,
    phase: &str,
    command: &Outgoing,
    result: &Json,
    unsupported: bool,
) -> Result<(), EngineError> {
    observation::record(job, phase_of(phase)?, command, result, unsupported).await
}
