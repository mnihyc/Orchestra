//! Source readiness, before-turn file recording and watcher-only end observations.
//!
//! Preparation happens before a turn or lease exists. Its immutable recording identity includes
//! the input, next turn number and current binding, so a restart finds the same captured files,
//! while a later turn or changed directory never borrows another before-state. Failed coverage
//! is persisted as an explained fallback. End observation uses the connector's prepared watcher
//! baseline only; it neither runs Git nor replaces the last explicitly requested Git work facts.

use std::collections::{BTreeMap, BTreeSet};

use futures::StreamExt;
use orch_api_contract::source::{SourceInfo, SourceState};
use orch_db::queries::environments::thread_engine as environments;
use orch_db::queries::threads::{InputRow, ThreadRow, TurnRow};
use orch_db::queries::{source as rows, storage, threads};
use orch_link_contract::commands::SerialKey;
use orch_link_contract::source::{
    FsRecord, FsRecordResult, RecordKind, RecordState, WorkspaceObserve, WorkspaceObserveResult,
};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::{Hash, Id};
use serde_json::json;

use crate::commands::{CommandsError, Outcome, Outgoing};
use crate::manifest::{Entry, Manifest};
use crate::unit::Unit;
use crate::{Engine, EngineError};

/// The first twenty included changed paths are the history's bounded preview.
const HISTORY_PATHS: usize = 20;

/// An existing directory is ready; a declared pending/failed source retains its waiting input.
pub(crate) fn ready(thread: &ThreadRow) -> Result<bool, EngineError> {
    thread
        .source
        .as_ref()
        .map(|source| {
            serde_json::from_value::<SourceInfo>(source.get("info").unwrap_or(source).clone())
                .map(|source| source.state == SourceState::Ready)
        })
        .transpose()
        .map(|ready| ready.unwrap_or(true))
        .map_err(Into::into)
}

fn identity(
    thread: &ThreadRow,
    input: Id,
    environment: &environments::EnvironmentRow,
    ordinal: i32,
) -> Id {
    crate::ids::derived(
        "source.before-state",
        &[
            thread.node_id.as_bytes(),
            input.as_bytes(),
            environment.id.as_bytes(),
            &environment.generation.to_le_bytes(),
            &ordinal.to_le_bytes(),
            thread.workspace_dir.as_bytes(),
        ],
    )
}

/// Whether this next turn already has either a complete state or a reasoned fallback.
pub(crate) async fn before_ready(
    unit: &mut Unit<'_>,
    thread: &ThreadRow,
    input: &InputRow,
) -> Result<bool, EngineError> {
    if !ready(thread)? {
        return Ok(false);
    }
    // The end of the thread's last turn is observed before another turn begins: after that turn's
    // commit, so its observation may still be due here.
    if rows::end_due_turn(&mut *unit.conn, thread.node_id)
        .await?
        .is_some()
    {
        return Ok(false);
    }
    let Some(environment) = thread.environment_id else {
        return Ok(false);
    };
    let Some(environment) = environments::environment(&mut *unit.conn, environment).await? else {
        return Ok(false);
    };
    let ordinal = rows::next_turn_ordinal(&mut *unit.conn, thread.node_id).await?;
    let state = storage::snapshot(
        &mut *unit.conn,
        identity(thread, input.id, &environment, ordinal),
    )
    .await?;
    Ok(state.is_some_and(|state| {
        matches!(state.state.as_str(), "complete" | "unavailable" | "expired")
    }))
}

fn unavailable(reason: &str) -> FsRecordResult {
    FsRecordResult {
        state: RecordState::Unavailable,
        tree_ref: None,
        repository_head: None,
        manifest_blob: None,
        files: 0,
        bytes: 0,
        expires_at: None,
        reason: Some(reason.to_owned()),
    }
}

fn recording_matches(
    manifest: &Manifest,
    generation: i32,
    root: &str,
    facts: &orch_types::Json,
    directory: &str,
) -> bool {
    let root = crate::sessions::workspace_root(root, facts, generation);
    let expected = crate::sessions::absolute_directory(root, directory);
    manifest.header().generation == i64::from(generation)
        && manifest.header().root == expected
        && manifest.entries().iter().all(complete_entry)
}

impl Engine {
    /// Repairs only a server-side unavailable fallback from its original successful command.
    /// No command is issued or repeated: today's files can never replace the captured before-state.
    pub async fn recover_recording(&self, snapshot: Id) -> Result<bool, EngineError> {
        let (saved, environment) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(saved) = storage::snapshot(&mut conn, snapshot).await? else {
                return Ok(false);
            };
            if saved.kind != "turn"
                || saved.storage != "environment"
                || saved.state != "unavailable"
                || saved.unavailable_reason.as_deref() != Some("store_unavailable")
                || saved.thread_id.is_none()
            {
                return Ok(false);
            }
            let Some(environment) =
                environments::environment(&mut conn, saved.environment_id).await?
            else {
                return Ok(false);
            };
            (saved, environment)
        };
        let Some(directory) = saved.directory.as_deref() else {
            return Ok(false);
        };
        if environment.generation != saved.env_generation {
            return Ok(false);
        }
        let command = crate::ids::derived("fs.record.before-turn", &[snapshot.as_bytes()]);
        let Some(body) = self.inner.commands.retained_body(command).await? else {
            return Ok(false);
        };
        let Ok(request) = serde_json::from_value::<FsRecord>(body) else {
            return Ok(false);
        };
        if request.snapshot_id != snapshot
            || request.environment_id != saved.environment_id
            || request.directory != directory
            || request.kind != RecordKind::Turn
            || request.record_days <= 0
            || request.max_bytes <= 0
        {
            return Ok(false);
        }
        let Some(info) = self.inner.commands.status(command).await? else {
            return Ok(false);
        };
        let Some(Outcome::Succeeded(value)) = info.outcome else {
            return Ok(false);
        };
        let Ok(result) = serde_json::from_value::<FsRecordResult>(value) else {
            return Ok(false);
        };
        if result.state != RecordState::Complete
            || result.files < 0
            || result.bytes < 0
            || result.tree_ref.as_deref().is_none_or(str::is_empty)
            || result
                .expires_at
                .is_none_or(|expires| expires.get() <= self.inner.clock.now())
        {
            return Ok(false);
        }
        let (Some(store), Some(hash)) = (self.blobs(), result.manifest_blob) else {
            return Ok(false);
        };
        let manifest = match self.read_manifest_checked(hash).await {
            Ok(manifest) => manifest,
            Err(error) => {
                tracing::warn!(%error, %snapshot, "the original successful recording still cannot be read");
                return Ok(false);
            }
        };
        if !recording_matches(
            &manifest,
            saved.env_generation,
            &environment.workspace_root,
            &environment.facts,
            directory,
        ) {
            return Ok(false);
        }
        let Some(size) = store.size(&hash).await.map_err(EngineError::unreadable)? else {
            return Ok(false);
        };
        // All store reads and command lookups ended before this short metadata-only transaction.
        let mut tx = self.inner.db.begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let Some(thread) = saved.thread_id else {
            return Ok(false);
        };
        if threads::lock(&mut *unit.conn, thread).await?.is_none() {
            return Ok(false);
        }
        let Some(current) = storage::snapshot(&mut *unit.conn, snapshot).await? else {
            return Ok(false);
        };
        let Some(current_env) =
            environments::environment(&mut *unit.conn, saved.environment_id).await?
        else {
            return Ok(false);
        };
        if current.environment_id != saved.environment_id
            || current.env_generation != saved.env_generation
            || current.thread_id != saved.thread_id
            || current.directory != saved.directory
            || current.kind != "turn"
            || current.storage != "environment"
            || current_env.generation != environment.generation
            || current_env.workspace_root != environment.workspace_root
            || result
                .expires_at
                .is_none_or(|expires| expires.get() <= unit.now())
        {
            return Ok(false);
        }
        let claimed = sqlx::query("UPDATE snapshots SET state='running' WHERE id=$1 AND state='unavailable' AND unavailable_reason='store_unavailable'")
            .bind(snapshot).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?.rows_affected() == 1;
        if !claimed {
            return Ok(false);
        }
        let now = unit.now();
        storage::register_blob(&mut *unit.conn, &hash, size, now).await?;
        if !rows::finish_recorded(&mut *unit.conn, snapshot, &result, now).await? {
            return Ok(false);
        }
        storage::pin_snapshot(&mut *unit.conn, snapshot, &[hash]).await?;
        let seq = unit.stamp(thread).await?;
        threads::set_seq_state(&mut *unit.conn, thread, seq).await?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(true)
    }

    /// Reserves and records files after attachments, then lets the ordinary preflight repeat its checks.
    pub(crate) async fn record_before(
        &self,
        thread: &ThreadRow,
        input: &InputRow,
        environment: &environments::EnvironmentRow,
    ) -> Result<(), EngineError> {
        let (snapshot, request) = {
            let mut tx = self.inner.db.begin().await?;
            let Some(current) = threads::lock(&mut tx, thread.node_id).await? else {
                return Ok(());
            };
            if current.environment_id != Some(environment.id)
                || current.workspace_dir != thread.workspace_dir
                || current.current_turn_id.is_some()
            {
                return Ok(());
            }
            let ordinal = rows::next_turn_ordinal(&mut tx, thread.node_id).await?;
            let snapshot = identity(&current, input.id, environment, ordinal);
            if storage::snapshot(&mut tx, snapshot)
                .await?
                .is_some_and(|state| state.state != "running")
            {
                return Ok(());
            }
            let days = registry::get(keys::WORKSPACE_RECORD_DAYS)
                .ok_or_else(|| EngineError::unreadable("recording days are not registered"))?;
            let bytes = registry::get(keys::WORKSPACE_RECORD_MAX_BYTES)
                .ok_or_else(|| EngineError::unreadable("recording bytes are not registered"))?;
            let settings = Snapshot::load(
                &mut tx,
                Place::Environment(environment.id),
                Wanted::Keys(&[days, bytes]),
            )
            .await?;
            let record_days = settings
                .resolve(days)
                .value
                .and_then(|value| value.as_i64())
                .ok_or_else(|| EngineError::unreadable("recording days are invalid"))?;
            let max_bytes = settings
                .resolve(bytes)
                .value
                .and_then(|value| value.as_i64())
                .ok_or_else(|| EngineError::unreadable("recording bytes are invalid"))?;
            rows::begin_recorded(
                &mut tx,
                &storage::NewSnapshot {
                    id: snapshot,
                    thread_id: Some(thread.node_id),
                    environment_id: environment.id,
                    env_generation: environment.generation,
                    label: "Before turn",
                    selection: &json!({ "include": [thread.workspace_dir], "exclude": [] }),
                    created_by: input.author_id,
                    at: self.inner.clock.now(),
                },
                &thread.workspace_dir,
                "turn",
            )
            .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            (
                snapshot,
                FsRecord {
                    environment_id: environment.id,
                    snapshot_id: snapshot,
                    directory: thread.workspace_dir.clone(),
                    kind: RecordKind::Turn,
                    record_days,
                    max_bytes,
                },
            )
        };
        let command_id = crate::ids::derived("fs.record.before-turn", &[snapshot.as_bytes()]);
        let mut result = if let Some(machine) = environment.machine_id {
            let command = Outgoing::new(
                command_id,
                machine,
                SerialKey::Environment(environment.id),
                &request,
            )?;
            let outcome = if self.inner.commands.status(command_id).await?.is_some() {
                self.inner.commands.wait(command_id).await
            } else {
                crate::commands::run(self.inner.commands.as_ref(), command).await
            };
            match outcome {
                Ok(Outcome::Succeeded(value)) => {
                    serde_json::from_value(value).unwrap_or_else(|error| {
                        tracing::warn!(%error, command=%command_id, environment=%environment.id,
                        "the file recording reply could not be read");
                        unavailable("store_unavailable")
                    })
                }
                Err(CommandsError::Stopping) => return Err(CommandsError::Stopping.into()),
                Ok(Outcome::Failed(error)) => {
                    tracing::warn!(code=%error.code, reason=%error.detail, command=%command_id, environment=%environment.id,
                        "the machine could not record the files before a turn");
                    unavailable("store_unavailable")
                }
                Ok(outcome @ (Outcome::Expired | Outcome::Cancelled)) => {
                    tracing::warn!(?outcome, command=%command_id, environment=%environment.id,
                        "the recording command ended before a complete answer");
                    unavailable("store_unavailable")
                }
                Err(error) => {
                    tracing::warn!(%error, command=%command_id, environment=%environment.id,
                        "the recording command could not complete");
                    unavailable("store_unavailable")
                }
            }
        } else {
            unavailable("offline")
        };
        if result.state == RecordState::Complete
            && result
                .expires_at
                .is_none_or(|expires| expires.get() <= self.inner.clock.now())
        {
            result = unavailable("expired");
        }
        let blob = if result.state == RecordState::Complete {
            match (
                self.blobs(),
                result.manifest_blob,
                result.tree_ref.as_ref(),
                result.expires_at,
            ) {
                (Some(store), Some(hash), Some(_), Some(_))
                    if result.files >= 0 && result.bytes >= 0 =>
                {
                    match self.read_manifest_checked(hash).await {
                        Ok(manifest)
                            if recording_matches(
                                &manifest,
                                environment.generation,
                                &environment.workspace_root,
                                &environment.facts,
                                &thread.workspace_dir,
                            ) =>
                        {
                            match store.size(&hash).await {
                                Ok(Some(size)) => Some((hash, size)),
                                Ok(None) => {
                                    tracing::warn!(command=%command_id, environment=%environment.id,
                                        "the recorded manifest is missing from the content store");
                                    None
                                }
                                Err(error) => {
                                    tracing::warn!(%error, command=%command_id, environment=%environment.id,
                                        "the recorded manifest size could not be read");
                                    None
                                }
                            }
                        }
                        Ok(manifest) => {
                            tracing::warn!(command=%command_id, environment=%environment.id,
                                expected_generation=environment.generation, generation=manifest.header().generation,
                                root_matches=manifest.header().root == crate::sessions::absolute_directory(crate::sessions::workspace_root(&environment.workspace_root, &environment.facts, environment.generation), &thread.workspace_dir),
                                "the recorded manifest did not match the environment or complete coverage");
                            None
                        }
                        Err(error) => {
                            tracing::warn!(%error, command=%command_id, environment=%environment.id,
                                "the recorded manifest could not be read from the content store");
                            None
                        }
                    }
                }
                _ => {
                    tracing::warn!(command=%command_id, environment=%environment.id, store_attached=self.blobs().is_some(),
                        "a complete file recording lacked its store or required metadata");
                    None
                }
            }
        } else {
            None
        };
        if result.state == RecordState::Complete && blob.is_none() {
            result = unavailable("store_unavailable");
        }
        if result.state == RecordState::Unavailable {
            result = unavailable(result.reason.as_deref().unwrap_or("store_unavailable"));
        }
        let mut tx = self.inner.db.begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let now = unit.now();
        if threads::lock(&mut *unit.conn, thread.node_id)
            .await?
            .is_none()
        {
            return Ok(());
        }
        if result.state == RecordState::Unavailable
            && request.record_days > 0
            && result.expires_at.is_none()
            && let Some(stored) = storage::snapshot(&mut *unit.conn, snapshot).await?
        {
            result.expires_at = chrono::Duration::try_days(request.record_days)
                .and_then(|age| stored.created_at.checked_add_signed(age))
                .map(orch_types::Time::new);
        }
        if let Some((hash, size)) = blob {
            storage::register_blob(&mut *unit.conn, &hash, size, now).await?;
        }
        if rows::finish_recorded(&mut *unit.conn, snapshot, &result, now).await? {
            if let Some((hash, _)) = blob {
                storage::pin_snapshot(&mut *unit.conn, snapshot, &[hash]).await?;
            }
            let seq = unit.stamp(thread.node_id).await?;
            threads::set_seq_state(&mut *unit.conn, thread.node_id, seq).await?;
            let mut entry = orch_db::audit::Entry::new(
                input
                    .author_id
                    .map_or(orch_db::audit::Actor::System, orch_db::audit::Actor::User),
                orch_db::audit::name!("workspace.state_recorded"),
                "environment",
                Some(environment.id),
                "Recorded the files before a turn",
            );
            entry.node_id = Some(thread.node_id);
            entry.org_id = Some(thread.org_id);
            entry.environment_id = Some(environment.id);
            entry.history = true;
            entry.details = json!({ "snapshot_id": snapshot, "thread_id": thread.node_id, "input_id": input.id,
                "bytes": result.bytes, "state": result.state, "reason": result.reason });
            orch_db::audit::append(
                &mut *unit.conn,
                &entry,
                &orch_db::audit::Origin::default(),
                now,
            )
            .await?;
        }
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Associates only the previously recorded next-turn state, after all the checks passed again.
    pub(crate) async fn bind_before(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input: &InputRow,
        environment: &environments::EnvironmentRow,
        turn: &TurnRow,
    ) -> Result<(), EngineError> {
        let snapshot = identity(thread, input.id, environment, turn.ordinal);
        if let Some(row) = storage::snapshot(&mut *unit.conn, snapshot).await?
            && row.state != "running"
        {
            let now = unit.now();
            if row.state == "complete"
                && row.expires_at.is_none_or(|expires| expires <= now)
                && let Some(Some(manifest)) =
                    rows::expire_recorded(&mut *unit.conn, snapshot, now).await?
            {
                storage::unpin_blobs(&mut *unit.conn, &[manifest], now).await?;
            }
            rows::set_before_state(&mut *unit.conn, turn.id, snapshot).await?;
        }
        Ok(())
    }

    /// Notes, in the transaction that ends `turn`, how its end is observed (no transaction waits for
    /// a machine or the content store). With nothing to ask (no segment, no environment of the turn's
    /// generation on a machine, no directory) the coverage is explicitly unknown at once; otherwise the
    /// observation is due, and [`Engine::drive_end_observation`] takes it once this transaction has
    /// committed. The thread's next turn waits for it ([`before_ready`]), so watcher end hashes are still
    /// stored before another turn, and a restart takes the same durable observation (the command's
    /// identifier is the turn's) rather than newer live files.
    pub(crate) async fn observe_turn_end(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        turn: &TurnRow,
        _seq: orch_types::Seq,
    ) -> Result<(), EngineError> {
        match self.end_plan(&mut *unit.conn, thread, turn).await? {
            EndPlan::Nothing => {}
            EndPlan::Ask(_) => rows::end_due(&mut *unit.conn, turn.id).await?,
            EndPlan::Unknown => {
                // Unknown coverage invalidates old observations conservatively.
                rows::set_end_manifest(&mut *unit.conn, turn.id, None, None).await?;
                rows::work_pending(&mut *unit.conn, turn.id, true).await?;
            }
        }
        Ok(())
    }

    /// What observing the end of `turn` takes, from the database alone.
    async fn end_plan(
        &self,
        conn: &mut orch_db::Conn,
        thread: &ThreadRow,
        turn: &TurnRow,
    ) -> Result<EndPlan, EngineError> {
        let Some(segment) = threads::segment(&mut *conn, turn.segment_id).await? else {
            return Ok(EndPlan::Nothing);
        };
        let environment = environments::environment(&mut *conn, segment.environment_id).await?;
        let before = match turn.before_snapshot_id {
            Some(id) => storage::snapshot(&mut *conn, id).await?,
            None => None,
        };
        let directory = before
            .as_ref()
            .and_then(|snapshot| snapshot.directory.clone())
            .or_else(|| {
                (thread.environment_id == Some(segment.environment_id))
                    .then(|| thread.workspace_dir.clone())
            });
        let Some(((environment, machine), directory)) = environment
            .filter(|environment| environment.generation == segment.env_generation)
            .and_then(|environment| {
                environment
                    .machine_id
                    .map(|machine| (environment.id, machine))
            })
            .zip(directory)
        else {
            return Ok(EndPlan::Unknown);
        };
        // No serial key, as the catalogue gives the kind: it reads the watcher's prepared baseline and
        // writes nothing, so it never waits behind another thread's recording or change of the
        // environment (C-turns: the end observations of a whole environment queued behind its
        // recordings, and the next turns behind them). This thread's own order is the engine's: its
        // next turn waits for this observation ([`before_ready`]).
        let command = Outgoing::new(
            crate::ids::derived("workspace.observe.end-turn", &[turn.id.as_bytes()]),
            machine,
            SerialKey::None,
            &WorkspaceObserve {
                environment_id: environment,
                thread_id: thread.node_id,
                turn_id: Some(turn.id),
                directory,
            },
        )?;
        Ok(EndPlan::Ask(Box::new(EndObservation {
            command,
            machine,
            environment: segment.environment_id,
            before,
        })))
    }

    /// Takes the end observation that a committed turn of `thread_id` left due ([`Engine::observe_turn_end`]):
    /// the machine is asked and the content store read with no transaction open, and what they said is
    /// written once, in a short transaction of its own under the thread's lock.
    pub(crate) async fn drive_end_observation(&self, thread_id: Id) -> Result<(), EngineError> {
        let (turn, plan) = {
            let mut conn = self.inner.db.acquire().await?;
            let Some(turn) = rows::end_due_turn(&mut conn, thread_id).await? else {
                return Ok(());
            };
            let Some(turn) = threads::turn(&mut conn, turn).await? else {
                return Ok(());
            };
            let Some(thread) = threads::thread(&mut conn, thread_id).await? else {
                return Ok(());
            };
            let plan = self.end_plan(&mut conn, &thread, &turn).await?;
            (turn, plan)
        };
        let (environment, before, observed) = match plan {
            EndPlan::Ask(observation) => {
                let EndObservation {
                    command,
                    machine,
                    environment,
                    before,
                } = *observation;
                let known = self.inner.commands.status(command.id).await?;
                let outcome = match known.and_then(|info| info.outcome) {
                    Some(outcome) => Ok(outcome),
                    None => {
                        let online = {
                            let mut conn = self.inner.db.acquire().await?;
                            orch_db::queries::resources::machine(&mut conn, machine)
                                .await?
                                .is_some_and(|machine| machine.online && machine.is_active())
                        };
                        if online {
                            crate::commands::run(self.inner.commands.as_ref(), command).await
                        } else {
                            Ok(Outcome::Expired)
                        }
                    }
                };
                // A missing or offline read is unknown coverage. A process restart instead retries
                // the same durable observation, rather than substituting the newer live files.
                let observed = match outcome {
                    Ok(Outcome::Succeeded(value)) => {
                        serde_json::from_value::<WorkspaceObserveResult>(value)
                            .ok()
                            .and_then(|value| value.end_manifest_blob)
                    }
                    Err(CommandsError::Stopping) => return Err(CommandsError::Stopping.into()),
                    _ => None,
                };
                (Some(environment), before, observed)
            }
            EndPlan::Nothing | EndPlan::Unknown => (None, None, None),
        };
        let mut end = match observed {
            Some(hash) => self
                .read_manifest_checked(hash)
                .await
                .ok()
                .filter(|manifest| manifest.entries().iter().all(complete_entry))
                .map(|manifest| (hash, manifest)),
            None => None,
        };
        let retained = match end.as_ref() {
            Some((hash, _)) => match self.blobs() {
                Some(store) => store
                    .size(hash)
                    .await
                    .ok()
                    .flatten()
                    .map(|size| (*hash, size)),
                None => None,
            },
            None => None,
        };
        if retained.is_none() {
            end = None;
        }
        let before = match (
            end.as_ref(),
            before
                .filter(|snapshot| snapshot.state == "complete")
                .and_then(|snapshot| snapshot.manifest_blob),
        ) {
            (Some(_), Some(hash)) => self.read_manifest_checked(hash).await.ok(),
            _ => None,
        };
        let mut tx = self.inner.db.begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let Some(thread) = threads::lock(&mut *unit.conn, thread_id).await? else {
            return Ok(());
        };
        if !rows::take_end_due(&mut *unit.conn, turn.id).await? {
            return Ok(());
        }
        // The turn's own moment, as when this was written in the transaction that ended it.
        let now = turn.completed_at.unwrap_or_else(|| unit.now());
        let seq = unit.stamp(thread_id).await?;
        if let Some((hash, size)) = retained {
            storage::register_blob(&mut *unit.conn, &hash, size, now).await?;
            storage::pin_blobs(&mut *unit.conn, &[hash]).await?;
        }
        let namespace = end.as_ref().map(|(_, manifest)| end_namespace(manifest));
        rows::set_end_manifest(
            &mut *unit.conn,
            turn.id,
            namespace.as_ref(),
            end.as_ref().map(|(hash, _)| hash),
        )
        .await?;
        // Unknown coverage also invalidates old observations conservatively. Only a complete,
        // equal before/end comparison can prove that no included change occurred.
        rows::work_pending(&mut *unit.conn, turn.id, true).await?;
        if let Some((_, end)) = end
            && let Some(before) = before
            && let Some(environment) = environment
            && before.header().root == end.header().root
            && before.header().generation == end.header().generation
        {
            let changes = changed_paths(&before, &end);
            rows::work_pending(&mut *unit.conn, turn.id, !changes.paths.is_empty()).await?;
            if !changes.paths.is_empty() {
                rows::stale_work(&mut *unit.conn, thread.node_id, now, seq).await?;
                // The thread is the row's object, so its place is the thread's: the audit takes an
                // object's own place over any other (`trace::facts`), and the environment's would hide
                // the row from the thread and from the replay of its invalidations, which find it by
                // the thread (`invalidate_work`). The environment's history reads it by
                // `environment_id`.
                let mut entry = orch_db::audit::Entry::new(
                    orch_db::audit::Actor::Agent(thread.node_id),
                    orch_db::audit::name!("workspace.changed_by_agent"),
                    "thread",
                    Some(thread.node_id),
                    "A turn changed included files",
                );
                entry.node_id = Some(thread.node_id);
                entry.org_id = Some(thread.org_id);
                entry.environment_id = Some(environment);
                entry.details = json!({ "thread_id": thread.node_id, "turn_id": turn.id, "files": changes.paths.len(),
                        "added": changes.added, "removed": changes.removed,
                        "paths": changes.paths.iter().take(HISTORY_PATHS).collect::<Vec<_>>(),
                        "more": changes.paths.len().saturating_sub(HISTORY_PATHS) });
                orch_db::audit::scope_agent_turn(
                    thread.node_id,
                    turn.id,
                    orch_db::audit::append(
                        &mut *unit.conn,
                        &entry,
                        &orch_db::audit::Origin::default(),
                        now,
                    ),
                )
                .await?;
            }
        }
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    pub(crate) async fn read_manifest_checked(&self, hash: Hash) -> Result<Manifest, EngineError> {
        let store = self
            .blobs()
            .ok_or_else(|| EngineError::unreadable("the manifest store is unavailable"))?;
        let read = store
            .get(&hash, None)
            .await
            .map_err(EngineError::unreadable)?;
        if read.meta.size < 0
            || read.meta.size as u64 > crate::manifest::MANIFEST_MAX_EXPANDED_BYTES
        {
            return Err(EngineError::unreadable("the end manifest is too large"));
        }
        let mut body = read.body;
        let mut bytes = Vec::new();
        while let Some(part) = body.next().await {
            let part = part.map_err(EngineError::unreadable)?;
            if bytes.len().saturating_add(part.len()) as u64
                > crate::manifest::MANIFEST_MAX_EXPANDED_BYTES
            {
                return Err(EngineError::unreadable("the end manifest is too large"));
            }
            bytes.extend_from_slice(&part);
        }
        if Hash::of(&bytes) != hash {
            return Err(EngineError::unreadable(
                "the end manifest failed its hash check",
            ));
        }
        Manifest::parse(&bytes).map_err(EngineError::unreadable)
    }
}

struct Changes {
    paths: Vec<String>,
    added: usize,
    removed: usize,
}

/// How the end of a turn is observed.
enum EndPlan {
    /// Not at all: the turn has no segment.
    Nothing,
    /// Unknown coverage: there is no machine or directory to ask.
    Unknown,
    /// By asking the machine, after the commit of the transaction that ended the turn.
    Ask(Box<EndObservation>),
}

struct EndObservation {
    command: Outgoing,
    machine: Id,
    environment: Id,
    before: Option<storage::SnapshotRow>,
}

/// Complete included names and raw versions, independent of when a manifest was encoded.
pub fn end_namespace(manifest: &Manifest) -> orch_types::Json {
    orch_types::Json::Object(
        manifest
            .entries()
            .iter()
            .map(|entry| {
                let hash = if entry.kind == crate::manifest::EntryKind::Link {
                    entry.link.as_deref().map(|link| Hash::of(link.as_bytes()))
                } else {
                    entry.hash
                };
                (
                    entry.path.clone(),
                    json!({ "h": hash, "t": entry.kind, "x": entry.mode & 0o111 }),
                )
            })
            .collect(),
    )
}

fn complete_entry(entry: &Entry) -> bool {
    match entry.kind {
        crate::manifest::EntryKind::File => entry.hash.is_some(),
        crate::manifest::EntryKind::Link => entry.link.is_some(),
        crate::manifest::EntryKind::Dir => true,
        crate::manifest::EntryKind::Other => false,
    }
}

fn same(left: &Entry, right: &Entry) -> bool {
    left.kind == right.kind
        && left.size == right.size
        && left.hash == right.hash
        && left.link == right.link
        && (left.mode & 0o111) == (right.mode & 0o111)
}

fn changed_paths(before: &Manifest, end: &Manifest) -> Changes {
    let before: BTreeMap<&str, &Entry> = before
        .entries()
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let end: BTreeMap<&str, &Entry> = end
        .entries()
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let names: BTreeSet<&str> = before.keys().chain(end.keys()).copied().collect();
    let mut changes = Changes {
        paths: Vec::new(),
        added: 0,
        removed: 0,
    };
    for path in names {
        match (before.get(path), end.get(path)) {
            (None, Some(_)) => changes.added += 1,
            (Some(_), None) => changes.removed += 1,
            (Some(before), Some(end)) if same(before, end) => continue,
            (Some(_), Some(_)) => {}
            (None, None) => continue,
        }
        changes.paths.push(path.to_owned());
    }
    changes
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    use crate::manifest::{EntryKind, Header};

    fn recorded(
        root: &str,
        generation: i64,
        complete: bool,
    ) -> Result<Manifest, crate::manifest::ManifestError> {
        let written = crate::manifest::build(
            &Header {
                v: crate::manifest::MANIFEST_VERSION,
                root: root.to_owned(),
                generated_at: orch_types::Time::new(chrono::Utc::now()),
                generation,
                case_sensitive: true,
            },
            vec![Entry {
                path: "file.txt".into(),
                kind: EntryKind::File,
                size: 1,
                mtime_ns: 0,
                mode: 0o600,
                hash: complete.then(|| Hash::of(b"x")),
                link: None,
            }],
        )?;
        Manifest::parse(&written.bytes)
    }

    #[test]
    fn a_recording_keeps_generation_directory_and_complete_coverage_with_a_reported_root()
    -> Result<(), crate::manifest::ManifestError> {
        let facts = json!({"workspace_root":"/actual","generation":4});
        let matches = |manifest: &Manifest, facts: &orch_types::Json| {
            recording_matches(manifest, 4, "/registered", facts, "web")
        };
        assert!(matches(&recorded("/actual/web", 4, true)?, &facts));
        assert!(!matches(&recorded("/other/web", 4, true)?, &facts));
        assert!(!matches(&recorded("/actual/web-other", 4, true)?, &facts));
        assert!(!matches(&recorded("/actual/web", 3, true)?, &facts));
        assert!(!matches(&recorded("/actual/web", 4, false)?, &facts));
        assert!(!matches(
            &recorded("/actual/web", 4, true)?,
            &json!({"workspace_root":"/actual","generation":3}),
        ));
        assert!(!matches(
            &recorded("actual/web", 4, true)?,
            &json!({"workspace_root":"actual","generation":4}),
        ));
        assert!(matches(&recorded("/registered/web", 4, true)?, &json!({})));
        Ok(())
    }
}
