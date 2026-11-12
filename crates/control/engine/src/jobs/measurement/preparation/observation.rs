//! Native answer witnesses are data; a new consumer supplies its own current complete source.
use super::*;
use crate::commands::{Outcome, RetainedCommand, Stage};
const WITNESS: &str = "measurement_observation";
const VERSION: &str = "measurement_observation_version";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Witness {
    version: u8,
    job: Id,
    physical: Hash,
    answers: BTreeMap<MeasurementPhase, Answer>,
    completed: Option<Completion>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Completion {
    at: chrono::DateTime<chrono::Utc>,
    state: String,
    result: Option<Hash>,
    error: Option<Hash>,
}
fn read_witness(row: &JobRow) -> Result<Option<Witness>, EngineError> {
    match (row.plan.get(WITNESS), row.plan.get(VERSION)) {
        (None, None) => Ok(None),
        (Some(value), Some(version)) if version == &json!(1) => {
            let witness: Witness = serde_json::from_value(value.clone()).map_err(|_| closed())?;
            if witness.version != 1
                || witness.job != row.id
                || witness.physical != row_physical(row)?
            {
                return Err(closed());
            }
            Ok(Some(witness))
        }
        _ => Err(closed()),
    }
}
fn completion(row: &JobRow) -> Result<Completion, EngineError> {
    Ok(Completion {
        at: row.finished_at.ok_or_else(closed)?,
        state: row.state.clone(),
        result: row
            .result
            .as_ref()
            .map(crate::jobs::command_identity::hash)
            .transpose()?,
        error: row
            .error
            .as_ref()
            .map(crate::jobs::command_identity::hash)
            .transpose()?,
    })
}
fn frame(command: Id, expected: &Outgoing) -> Result<Hash, EngineError> {
    crate::jobs::command_identity::hash(&crate::jobs::command_identity::canonical(
        command,
        expected.machine,
        expected.key.wire().as_deref(),
        expected.kind.as_str(),
        &expected.body,
    )?)
}
fn same_wire(expected: &Outgoing, actual: &RetainedCommand) -> bool {
    actual.machine == expected.machine
        && actual.kind == expected.kind.as_str()
        && actual.key == expected.key.wire()
        && actual.body == expected.body
}
fn answer(
    command: Id,
    expected: &Outgoing,
    result: &Json,
    unsupported: bool,
) -> Result<Answer, EngineError> {
    Ok(Answer {
        command,
        machine: expected.machine,
        body: crate::jobs::command_identity::hash(&expected.body)?,
        result: crate::jobs::command_identity::hash(result)?,
        fingerprint: if expected.kind == CommandKind::NodeMountFingerprint && !unsupported {
            Some(serde_json::from_value::<NodeMountFingerprintResult>(result.clone())?.fingerprint)
        } else {
            None
        },
        unsupported,
        frame: Some(Frame {
            base: expected.id,
            canonical: frame(command, expected)?,
        }),
    })
}
pub(super) async fn record(
    job: &Job,
    phase: MeasurementPhase,
    supplied: &Outgoing,
    result: &Json,
    unsupported: bool,
) -> Result<(), EngineError> {
    let engine = job.engine();
    let snapshot = job.row().await?;
    let expected = build_command(
        &snapshot,
        phase,
        snapshot.started_at.unwrap_or(snapshot.created_at),
    )?;
    if expected.id != supplied.id
        || expected.machine != supplied.machine
        || expected.kind != supplied.kind
        || expected.key != supplied.key
        || expected.body != supplied.body
    {
        return Err(closed());
    }
    // The native runner clears active_command when its actual command ends. A
    // completed observation uses that exact canonical receipt, not an activity
    // marker as proof. An explicit still-active frame must retain its identity.
    let active = match snapshot.plan.get("active_command") {
        None | Some(Json::Null) => expected.id,
        Some(active) => serde_json::from_value(active.get("id").cloned().ok_or_else(closed)?)?,
    };
    let retained = engine
        .inner
        .commands
        .retained_command(active)
        .await?
        .ok_or_else(closed)?;
    if !same_wire(&expected, &retained) {
        return Err(closed());
    }
    let actual = engine
        .inner
        .commands
        .status(active)
        .await?
        .ok_or_else(closed)?;
    if actual.stage != Stage::Over {
        return Err(closed());
    }
    let native = match (actual.outcome, unsupported) {
        (Some(Outcome::Succeeded(value)), false) => value,
        (Some(Outcome::Failed(error)), true) if error.code == "unsupported" => {
            serde_json::to_value(error)?
        }
        _ => return Err(closed()),
    };
    if native != *result {
        return Err(closed());
    }
    let receipt = answer(active, &expected, &native, unsupported)?;
    let mut tx = engine.db().begin().await?;
    let row = job.lock_owned(&mut tx).await?.ok_or_else(closed)?;
    if row.kind != KIND
        || row_physical(&row)? != row_physical(&snapshot)?
        || row.plan.get("active_command") != snapshot.plan.get("active_command")
        || super::super::super::runner::read_progress(&row).0.phase != phase.as_str()
    {
        return Err(closed());
    }
    let mut witness = read_witness(&row)?.unwrap_or(Witness {
        version: 1,
        job: row.id,
        physical: row_physical(&row)?,
        answers: BTreeMap::new(),
        completed: None,
    });
    if witness.completed.is_some() {
        return Err(closed());
    }
    if let Some(old) = witness.answers.get(&phase)
        && serde_json::to_value(old)? != serde_json::to_value(&receipt)?
    {
        return Err(closed());
    }
    witness.answers.insert(phase, receipt);
    rows::merge_plan(
        &mut tx,
        row.id,
        &json!({WITNESS:witness,VERSION:1}),
        engine.inner.clock.now(),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
/// Run in the existing terminal transaction, after the actual job outcome was written.
pub(crate) async fn seal_finished(conn: &mut Conn, job: Id) -> Result<(), EngineError> {
    let row = rows::job(conn, job).await?.ok_or_else(closed)?;
    if row.kind != KIND {
        return Ok(());
    }
    let Some(mut witness) = read_witness(&row)? else {
        return Ok(());
    };
    if !answered(&row, &witness.answers)? {
        return Ok(());
    }
    let complete = completion(&row)?;
    if witness
        .completed
        .as_ref()
        .is_some_and(|old| old != &complete)
    {
        return Err(closed());
    }
    witness.completed = Some(complete);
    rows::merge_plan(
        conn,
        row.id,
        &json!({WITNESS:witness,VERSION:1}),
        row.updated_at,
    )
    .await?;
    Ok(())
}
fn terminal(row: &JobRow, witness: &Witness) -> Result<bool, EngineError> {
    let Some(seal) = &witness.completed else {
        return Ok(false);
    };
    if seal.at != row.finished_at.ok_or_else(closed)?
        || seal.state != row.state
        || seal.result
            != row
                .result
                .as_ref()
                .map(crate::jobs::command_identity::hash)
                .transpose()?
        || seal.error
            != row
                .error
                .as_ref()
                .map(crate::jobs::command_identity::hash)
                .transpose()?
        || witness
            .answers
            .values()
            .any(|answer| answer.frame.is_none())
        || !answered(row, &witness.answers)?
    {
        return Ok(false);
    }
    if row.state == "succeeded" {
        let result = row.result.as_ref().ok_or_else(closed)?;
        let kind: CommandKind = row.plan["command_kind"]
            .as_str()
            .ok_or_else(closed)?
            .parse()
            .map_err(|_| closed())?;
        if kind == CommandKind::NodeMountFingerprint {
            let _: orch_api_contract::forks::PlanMountFingerprints =
                serde_json::from_value(result.clone())?;
        } else {
            validate_count(kind, &row.plan["body"], result)?;
        }
        Ok(true)
    } else {
        Ok(row.state == "failed"
            && row.error.as_ref().and_then(|error| error["code"].as_str()) == Some("unsupported"))
    }
}
pub(super) fn terminal_witness(row: &JobRow) -> Result<Option<bool>, EngineError> {
    read_witness(row)?
        .map(|witness| terminal(row, &witness))
        .transpose()
}
async fn receipt_matches(
    conn: &mut Conn,
    row: &JobRow,
    witness: &Witness,
) -> Result<bool, EngineError> {
    for (phase, answer) in &witness.answers {
        let expected = build_command(row, *phase, row.started_at.unwrap_or(row.created_at))?;
        if let Some(actual) = orch_db::queries::link::command(conn, answer.command).await? {
            let retained = RetainedCommand {
                machine: actual.machine_id,
                kind: actual.kind.clone(),
                key: actual.serial_key().map(str::to_owned),
                body: actual.body.clone(),
            };
            if !same_wire(&expected, &retained)
                || actual.state
                    != if answer.unsupported {
                        "failed"
                    } else {
                        "succeeded"
                    }
                || actual
                    .result
                    .as_ref()
                    .filter(|_| !answer.unsupported)
                    .or(actual.error.as_ref().filter(|_| answer.unsupported))
                    .map(crate::jobs::command_identity::hash)
                    .transpose()?
                    .is_some_and(|hash| hash != answer.result)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
/// Compatibility reads actual surviving command receipts, never a terminal job marker.
async fn from_receipts(conn: &mut Conn, row: &JobRow) -> Result<Option<Witness>, EngineError> {
    let kind: CommandKind = row.plan["command_kind"]
        .as_str()
        .ok_or_else(closed)?
        .parse()
        .map_err(|_| closed())?;
    let mut witness = Witness {
        version: 1,
        job: row.id,
        physical: row_physical(row)?,
        answers: BTreeMap::new(),
        completed: Some(completion(row)?),
    };
    for phase in phases_for(kind) {
        let expected = build_command(row, phase, row.started_at.unwrap_or(row.created_at))?;
        let Some(actual) = orch_db::queries::link::command(conn, expected.id).await? else {
            return Ok(None);
        };
        let retained = RetainedCommand {
            machine: actual.machine_id,
            kind: actual.kind.clone(),
            key: actual.serial_key().map(str::to_owned),
            body: actual.body.clone(),
        };
        if !same_wire(&expected, &retained) {
            return Ok(None);
        }
        let (result, unsupported) = match actual.state.as_str() {
            "succeeded" => match actual.result {
                Some(result) => (result, false),
                None => return Ok(None),
            },
            "failed" => match actual.error {
                Some(error) if error["code"] == json!("unsupported") => (error, true),
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        witness
            .answers
            .insert(phase, answer(expected.id, &expected, &result, unsupported)?);
    }
    Ok(Some(witness))
}
fn valid(
    row: &JobRow,
    request: &Request,
    reference: &Measurement,
    clock: &dyn Clock,
    seconds: i64,
) -> Result<bool, EngineError> {
    Ok(row.kind == KIND
        && row_physical(row)? == request.physical()?
        && row.finished_at.is_some_and(|at| {
            at <= clock.now()
                && (request.kind == CommandKind::NodeMountFingerprint
                    || at > clock.now() - Duration::seconds(seconds))
        })
        && reference
            .measured_at
            .is_none_or(|at| row.finished_at.map(Time::new) == Some(at))
        && (row.state == "succeeded"
            || request.reuse_unsupported
                && row.state == "failed"
                && row.error.as_ref().and_then(|error| error["code"].as_str())
                    == Some("unsupported")))
}
fn pending(row: &JobRow, request: &Request, reference: &Measurement) -> Result<bool, EngineError> {
    if row.kind != KIND
        || !row.is_open()
        || row.cancel_requested()
        || row.finished_at.is_some()
        || row.result.is_some()
        || row.error.is_some()
        || reference.measured_at.is_some()
        || row_physical(row)? != request.physical()?
    {
        return Ok(false);
    }
    let stored = read(row)?;
    let mut identity = json!([
        row.plan["machine"],
        row.plan["command_kind"],
        row.plan["body"],
        row.plan["subject"]
    ]);
    if let Some(stored) = stored {
        identity
            .as_array_mut()
            .ok_or_else(closed)?
            .push(json!(["native_source_v1", stored.original]));
    }
    if row.plan["measurement_key"].as_str()
        != Some(
            crate::jobs::command_identity::hash(&identity)?
                .to_string()
                .as_str(),
        )
    {
        return Ok(false);
    }
    for phase in phases_for(request.kind) {
        let _ = build_command(row, phase, row.started_at.unwrap_or(row.created_at))?;
    }
    Ok(true)
}
struct ReferenceRead {
    observation: ReferenceObservation,
    accepted: Accepted,
    valid_until: Option<Time>,
}
async fn read_on(
    engine: &Engine,
    conn: &mut Conn,
    request: &Request,
    reference: &Measurement,
    source: &Preparation,
) -> Result<Option<ReferenceRead>, EngineError> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        return Err(closed());
    }
    let first = source.accept(engine, conn, request, None).await?;
    if first.actor() != request.by {
        return Err(closed());
    }
    let _ = sqlx::query("SELECT id FROM jobs WHERE id=$1 FOR SHARE")
        .bind(reference.job_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    let Some(row) = rows::job(conn, reference.job_id).await? else {
        return Ok(None);
    };
    let pending = pending(&row, request, reference)?;
    let seconds = setting(conn, keys::MEASUREMENT_VALID_SECS).await?;
    if !pending
        && !valid(
            &row,
            request,
            reference,
            engine.inner.clock.as_ref(),
            seconds,
        )?
    {
        return Ok(None);
    }
    let witness = if pending {
        if read_witness(&row)?.is_some_and(|witness| witness.completed.is_some()) {
            return Ok(None);
        }
        None
    } else {
        let witness = match read_witness(&row)? {
            Some(witness) => witness,
            None => match from_receipts(conn, &row).await? {
                Some(witness) => witness,
                None => return Ok(None),
            },
        };
        if !terminal(&row, &witness)? || !receipt_matches(conn, &row, &witness).await? {
            return Ok(None);
        }
        Some(witness)
    };
    source.check_observation_request(engine, conn, &row).await?;
    let accepted = first.refresh(engine, conn, request, None).await?;
    if accepted.actor() != request.by || !accepted.origin.admits(&accepted.caller) {
        return Err(closed());
    }
    accepted.check(engine.inner.clock.as_ref())?;
    if if pending {
        !self::pending(&row, request, reference)?
    } else {
        !valid(
            &row,
            request,
            reference,
            engine.inner.clock.as_ref(),
            seconds,
        )?
    } {
        return Ok(None);
    }
    let count = Count {
        reference: Measurement {
            job_id: row.id,
            measured_at: row
                .finished_at
                .filter(|_| row.state == "succeeded")
                .map(Time::new),
        },
        result: if row.state == "succeeded" {
            row.result.clone()
        } else {
            None
        },
        unsupported: !pending && row.state == "failed",
        origin: accepted
            .origin
            .complete()
            .cloned()
            .map(|origin| (origin, accepted.until)),
    };
    let observation = Observation {
        request: request.hash()?,
        association: source.identity()?,
        count,
        witness: witness
            .as_ref()
            .map(|witness| crate::jobs::command_identity::hash(&serde_json::to_value(witness)?))
            .transpose()?,
    };
    let valid_until = if request.kind == CommandKind::NodeMountFingerprint {
        None
    } else {
        row.finished_at
            .map(|at| Time::new(at + Duration::seconds(seconds)))
    };
    let observation = if pending {
        ReferenceObservation::Pending(PendingReference(observation))
    } else {
        ReferenceObservation::Complete(observation)
    };
    Ok(Some(ReferenceRead {
        observation,
        accepted,
        valid_until,
    }))
}
impl Engine {
    /// Read one completed native observation as data under a NEW current consumer Source.
    /// This never claims, renews, adopts or reissues the old measurement.
    pub async fn completed_measurement_observation_on(
        &self,
        conn: &mut Conn,
        request: &Request,
        reference: &Measurement,
        source: &Preparation,
    ) -> Result<Option<Observation>, EngineError> {
        Ok(self
            .measurement_reference_observation_on(conn, request, reference, source)
            .await?
            .and_then(ReferenceObservation::completed))
    }
    /// The same data-only read on a short owned transaction, including its post-commit lifetime tail.
    pub async fn completed_measurement_observation(
        &self,
        request: &Request,
        reference: &Measurement,
        source: &Preparation,
    ) -> Result<Option<Observation>, EngineError> {
        Ok(self
            .measurement_reference_observation(request, reference, source)
            .await?
            .and_then(ReferenceObservation::completed))
    }

    /// Read the actual saved reference as pending or proven completed data under
    /// this new current Source, without claiming/renewing/adopting/reissuing it.
    pub async fn measurement_reference_observation_on(
        &self,
        conn: &mut Conn,
        request: &Request,
        reference: &Measurement,
        source: &Preparation,
    ) -> Result<Option<ReferenceObservation>, EngineError> {
        Ok(read_on(self, conn, request, reference, source)
            .await?
            .map(|read| read.observation))
    }
    /// The same typed reference read with its real post-commit lifetime tail.
    pub async fn measurement_reference_observation(
        &self,
        request: &Request,
        reference: &Measurement,
        source: &Preparation,
    ) -> Result<Option<ReferenceObservation>, EngineError> {
        let mut tx = self.db().begin().await?;
        let result = read_on(self, &mut tx, request, reference, source).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if let Some(read) = &result {
            read.accepted.check(self.inner.clock.as_ref())?;
            if read
                .valid_until
                .is_some_and(|until| self.inner.clock.time() >= until)
            {
                return Ok(None);
            }
        }
        Ok(result.map(|read| read.observation))
    }
}
