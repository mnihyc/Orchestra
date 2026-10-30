//! Lost environments are provisioned again on reviewed owned volumes or verified cached workspace bytes.

use crate::{
    Engine, EngineError,
    commands::{Outcome, Outgoing},
    ids,
    manifest::{self, EntryKind},
    places,
    updates::{self, UpdateError},
};
use orch_api_contract::{
    environment::PlaceHost,
    forks::{FilesMissing, FilesPlan, FilesSource, PathMap},
};
use orch_authz::Principal;
use orch_db::{
    Conn, DbError,
    queries::{
        environments::{hosting as envs, workspace},
        jobs,
        resources::hosting as hosts,
        storage,
    },
};
use orch_link_contract::commands::{NodeVolumeInspect, NodeVolumeInspectResult, SerialKey};
use orch_types::{Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};

/// Frozen inputs for provisioning the same environment after permanent loss.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// The environment that keeps its page and threads.
    pub environment: Id,
    /// The reviewed source revision.
    pub revision: i64,
    /// Who authorized the new placement.
    pub by: Id,
    /// The chosen host.
    pub host: Id,
    /// The reviewed template.
    pub template: Id,
    /// The reviewed definition preserving the old runtime's durable network restriction.
    pub definition: Json,
    /// The frozen egress policy, never widened by recovery.
    pub network_policy: orch_driver_contract::network::NetworkPolicy,
    /// Exact workspace and home volume names to check and reuse or create.
    pub volumes: Json,
    /// The roles found restored with matching protected ownership.
    pub restored: BTreeMap<String, Option<i64>>,
    /// The manifest of verified cached files, absent when there is no listing.
    pub manifest: Option<Hash>,
    /// Whether the source listing was available.
    pub listing_available: bool,
    /// Every known file that cannot be recovered from Orch.
    pub missing: Vec<String>,
}

/// A recreation plan, with the destination and recoverable coverage shown before confirmation.
pub struct Draft {
    /// The durable provisioning inputs.
    pub recipe: Recipe,
    /// The destination's measured and declared placement facts.
    pub host: PlaceHost,
    /// Recoverable workspace coverage.
    pub files: FilesPlan,
}

/// Database-checked placement captured before any native measurement begins.
pub struct Prepared {
    environment: Id,
    revision: i64,
    by: Id,
    host: PlaceHost,
    host_id: Id,
    machine: Id,
    template: Id,
    driver: String,
    definition: Json,
    network_policy: orch_driver_contract::network::NetworkPolicy,
    volumes: Json,
    workspace_root: String,
}

impl Prepared {
    /// The exact host whose use the caller has checked.
    #[must_use]
    pub fn host(&self) -> Id {
        self.host_id
    }
    /// The exact template whose use the caller has checked.
    #[must_use]
    pub fn template(&self) -> Id {
        self.template
    }
}

impl Engine {
    /// Plans a lost environment on a usable host without changing any runtime or volume.
    pub async fn prepare_recovery(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        environment: Id,
        host: Option<Id>,
    ) -> Result<Prepared, UpdateError> {
        if !updates::may_manage(conn, principal, updates::Target::Environment(environment)).await? {
            return Err(UpdateError::Denied);
        }
        let state = envs::environment(conn, environment)
            .await?
            .filter(|row| row.status == "lost" && row.deleted_at.is_none())
            .ok_or_else(|| {
                UpdateError::Blocked(vec![updates::blocker(
                    "environment_not_lost",
                    "Only a lost live environment can be re-created",
                )])
            })?;
        let template = match state.template_id {
            Some(id) => Some(id),
            None => orch_settings::get::<Id>(
                conn,
                orch_settings::Place::Environment(environment),
                orch_settings_contract::registry::keys::ENV_TEMPLATE_ID,
            )
            .await
            .map_err(EngineError::from)?,
        }
        .ok_or_else(|| {
            UpdateError::Blocked(vec![updates::blocker(
                "template_required",
                "Choose an environment template before re-creating this directory",
            )])
        })?;
        let mut template = hosts::template(conn, template)
            .await?
            .ok_or(UpdateError::NotFound)?;
        let required:Option<String>=sqlx::query_scalar::<_,Option<String>>("SELECT network_required FROM runtimes WHERE environment_id=$1 AND lost_at IS NOT NULL ORDER BY generation DESC,created_at DESC LIMIT 1").bind(environment).fetch_optional(&mut *conn).await.map_err(DbError::from)?.flatten();
        if required.as_deref() == Some("restricted") && template.spec["network"]["mode"] != "none" {
            template.spec["network"] = json!({"mode":"bridge","egress":"restricted"});
        }
        let network_policy = crate::network::policy(
            conn,
            orch_settings::Place::Environment(environment),
            &template.spec,
        )
        .await?;
        let wanted =
            places::wanted(conn, state.home_node_id, template.clone(), &state.config).await?;
        let choices = places::host_places_for_environment(
            conn,
            principal,
            &places::Ask {
                home: state.home_node_id,
                wanted: &wanted,
                current: None,
                minutes_idle: places::IDLE_MINUTES_DEFAULT,
                now: self.inner.clock.now(),
            },
            environment,
        )
        .await?;
        let chosen = places::choose(&choices, host).map_err(|reason| {
            UpdateError::Blocked(vec![updates::blocker(reason.code, reason.message)])
        })?;
        let old:Option<Json>=sqlx::query_scalar("SELECT volumes FROM runtimes WHERE environment_id=$1 AND lost_at IS NOT NULL ORDER BY generation DESC,created_at DESC LIMIT 1").bind(environment).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        let mut volumes=old.unwrap_or_else(||json!({"workspace":{"name":format!("orch-ws-{environment}"),"storage":null},"home":{"name":format!("orch-home-{environment}"),"storage":null}}));
        for (role, prefix) in [("workspace", "orch-ws"), ("home", "orch-home")] {
            if volumes.get(role).is_none() || volumes[role].get("directory").is_some() {
                volumes[role] = json!({"name":format!("{prefix}-{environment}"),"storage":null});
            }
        }
        Ok(Prepared {
            environment,
            revision: state.revision,
            by: principal.user(),
            host: chosen.place.clone(),
            host_id: chosen.host.id,
            machine: chosen.host.machine_id,
            template: template.id,
            driver: template.driver,
            definition: template.spec,
            network_policy,
            volumes,
            workspace_root: state.workspace_root,
        })
    }

    /// Measures only the reviewed destination, after the caller releases its planning transaction.
    pub async fn measure_recovery(&self, prepared: Prepared) -> Result<Draft, UpdateError> {
        let Prepared {
            environment,
            revision,
            by,
            host,
            host_id,
            machine,
            template,
            driver,
            definition,
            network_policy,
            mut volumes,
            workspace_root,
        } = prepared;
        let mut restored = BTreeMap::new();
        let roles = volumes
            .as_object()
            .ok_or_else(|| EngineError::unreadable("the recorded volume map is invalid"))?
            .iter()
            .filter(|(role, volume)| role.as_str() != "state" && volume.get("directory").is_none())
            .map(|(role, _)| role.clone())
            .collect::<Vec<_>>();
        for role in roles {
            let request = NodeVolumeInspect {
                driver: driver.clone(),
                name: volumes[&role]["name"]
                    .as_str()
                    .ok_or_else(|| EngineError::unreadable("the recorded volume has no name"))?
                    .into(),
                storage: volumes[&role]["storage"].as_str().map(str::to_owned),
                environment_id: environment,
            };
            let answer = self
                .ask_bounded(
                    Outgoing::new(
                        orch_types::new_id(),
                        machine,
                        SerialKey::Volume(request.name.clone()),
                        &request,
                    )
                    .map_err(EngineError::from)?,
                )
                .await?;
            let result: NodeVolumeInspectResult = match answer {
                Some(Outcome::Succeeded(value)) => {
                    serde_json::from_value(value).map_err(EngineError::from)?
                }
                _ => {
                    return Err(UpdateError::Blocked(vec![updates::blocker(
                        "restored_volume_unavailable",
                        "The target could not verify restored volume ownership",
                    )]));
                }
            };
            if result.bytes.is_some_and(|bytes| bytes < 0) {
                return Err(UpdateError::Blocked(vec![updates::blocker(
                    "restored_volume_unavailable",
                    "The target returned an invalid restored size",
                )]));
            }
            if result.exists {
                restored.insert(role, result.bytes);
            }
        }
        // A restored connector credential remains revoked; the new unit enrolls with a fresh state volume.
        volumes
            .as_object_mut()
            .ok_or_else(|| EngineError::unreadable("the recorded volume map is invalid"))?
            .remove("state");
        let mut connection = self.db().acquire().await?;
        let conn = &mut *connection;
        let listing = workspace::environment(conn, environment)
            .await?
            .and_then(|row| row.manifest_blob);
        let mut missing = Vec::new();
        let mut total_bytes = 0_i64;
        let mut file_count = 0_i64;
        let mut downloadable = 0_i64;
        let mut source_as_of = Time::new(self.inner.clock.now());
        let manifest = if let Some(hash) = listing {
            let original = self.read_manifest(&hash).await?;
            source_as_of = original.header().generated_at;
            let held = storage::blobs_among(conn, &original.hashes().collect::<Vec<_>>())
                .await?
                .into_iter()
                .collect::<HashSet<_>>();
            let store = self
                .blobs()
                .ok_or_else(|| EngineError::unreadable("the blob store is unavailable"))?;
            let mut entries = Vec::new();
            for entry in original.entries() {
                if entry.kind == EntryKind::File {
                    file_count += 1;
                    total_bytes = total_bytes.saturating_add(entry.size);
                    let available =
                        if let Some(hash) = entry.hash.filter(|hash| held.contains(hash)) {
                            store.size(&hash).await.ok().flatten() == Some(entry.size)
                        } else {
                            false
                        };
                    if !available {
                        if !restored.contains_key("workspace") {
                            missing.push(entry.path.clone());
                        }
                        continue;
                    }
                    downloadable = downloadable.saturating_add(entry.size);
                }
                if entry.kind != EntryKind::Other {
                    entries.push(entry.clone());
                }
            }
            let written =
                manifest::build(original.header(), entries).map_err(EngineError::unreadable)?;
            Some(self.put_manifest(conn, written.bytes).await?.hash)
        } else {
            None
        };
        let files = FilesPlan {
            kept_image_bytes: None,
            source: FilesSource::Cached,
            source_as_of,
            file_count,
            total_bytes,
            bytes_to_upload: 0,
            bytes_to_download: if restored.contains_key("workspace") {
                0
            } else {
                downloadable
            },
            missing: FilesMissing {
                count: missing.len() as i64,
                bytes: None,
                sample: missing
                    .iter()
                    .take(crate::fork_plan::MISSING_SAMPLE)
                    .cloned()
                    .collect(),
            },
            path_map: PathMap {
                from: workspace_root.clone(),
                to: "/workspace".into(),
            },
            home: None,
            private_views: Vec::new(),
            file_states: None,
        };
        Ok(Draft {
            recipe: Recipe {
                environment,
                revision,
                by,
                host: host_id,
                template,
                definition,
                network_policy,
                volumes,
                restored,
                manifest,
                listing_available: listing.is_some(),
                missing,
            },
            host,
            files,
        })
    }
}

/// Accepts the reviewed lost environment as an ordinary independently limited provisioning job.
pub async fn queue(
    engine: &Engine,
    conn: &mut Conn,
    recipe: &Recipe,
    batch: Option<Id>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<jobs::JobRow, UpdateError> {
    let environment = envs::lock_environment(conn, recipe.environment)
        .await?
        .filter(|row| row.status == "lost" && row.revision == recipe.revision)
        .ok_or(UpdateError::NotFound)?;
    if let Some(job) = jobs::changes::environment_hold(conn, environment.id).await? {
        return Err(UpdateError::Held(job.id));
    }
    sqlx::query("UPDATE environments SET placement='managed',template_id=$2,config=jsonb_set(config,'{env.data_sources}',$3) WHERE id=$1").bind(environment.id).bind(recipe.template).bind(json!({"workspace":{"kind":"made_by_orch"},"home":{"kind":"made_by_orch"}})).execute(&mut *conn).await.map_err(DbError::from)?;
    let mut job = crate::jobs::provision::queue(
        conn,
        now,
        &environment,
        crate::jobs::provision::Mode::Make,
        Some(recipe.by),
    )
    .await
    .map_err(|error| UpdateError::Internal(EngineError::unreadable(error)))?;
    let patch = json!({"recovery":recipe,"definition":recipe.definition,"network_policy":recipe.network_policy,"threads":envs::threads_of(conn,environment.id).await?});
    jobs::merge_plan(conn, job.id, &patch, now).await?;
    if let Some(batch) = batch {
        sqlx::query("UPDATE jobs SET batch_id=$2 WHERE id=$1")
            .bind(job.id)
            .bind(batch)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
        job.batch_id = Some(batch);
    }
    if let Some(manifest) = recipe.manifest {
        let mut hashes = engine
            .read_manifest(&manifest)
            .await?
            .hashes()
            .collect::<Vec<_>>();
        hashes.push(manifest);
        // The manifest's files are already held by the source environment's cache; the accepted listing itself is pinned until this job ends.
        storage::pin_blobs(conn, &hashes).await?;
        jobs::merge_plan(conn, job.id, &json!({"carry_pins":hashes}), now).await?;
    }
    Ok(job)
}

/// Copies only cached bytes into a fresh workspace and writes the missing-file record and history files.
pub(crate) async fn files(
    job: &crate::jobs::runner::Job,
    machine: Id,
    root: &str,
    generation: i32,
) -> Result<crate::jobs::runner::Done, crate::jobs::runner::Stop> {
    use crate::jobs::runner::{Done, Failure, Stop};
    use orch_link_contract::commands::{FsMaterialize, FsMaterializeResult, MaterializePurpose};
    let plan = job.plan().await?;
    let recipe: Recipe =
        serde_json::from_value(plan["recovery"].clone()).map_err(EngineError::from)?;
    if !recipe.restored.contains_key("workspace")
        && let Some(manifest) = recipe.manifest
    {
        let value = job
            .command(
                Outgoing::new(
                    ids::derived("recovery.workspace", &[job.id().as_bytes()]),
                    machine,
                    SerialKey::Environment(recipe.environment),
                    &FsMaterialize {
                        work: None,
                        environment_id: recipe.environment,
                        job_id: Some(job.id()),
                        manifest_blob: manifest,
                        target_root: root.into(),
                        purpose: MaterializePurpose::Move,
                    },
                )
                .map_err(EngineError::from)?,
            )
            .await?;
        let copied: FsMaterializeResult =
            serde_json::from_value(value).map_err(EngineError::from)?;
        let expected = job
            .engine()
            .read_manifest(&manifest)
            .await?
            .entries()
            .iter()
            .filter(|entry| entry.kind == EntryKind::File)
            .count() as i64;
        if copied.files + copied.skipped < expected {
            return Err(Stop::Failed(Failure::new(
                "files_missing",
                "The target did not verify every promised cached file",
            )));
        }
    }
    if !recipe.restored.contains_key("workspace")
        && (!recipe.missing.is_empty() || !recipe.listing_available)
    {
        let text = if recipe.listing_available {
            recipe.missing.join("\n") + "\n"
        } else {
            "Workspace listing unavailable; unlisted files could not be recovered.\n".into()
        };
        materialize_artifacts(
            job,
            machine,
            recipe.environment,
            &format!("{}/.orch", root.trim_end_matches('/')),
            generation,
            recipe.environment,
            vec![(
                "handoff/missing-files.txt".into(),
                Content::Bytes(text.into_bytes()),
            )],
        )
        .await?;
    }
    let thread_ids = {
        let mut conn = job.engine().db().acquire().await?;
        envs::threads_of(&mut conn, recipe.environment).await?
    };
    for thread in thread_ids {
        let mut conn = job.engine().db().acquire().await?;
        let row = orch_db::queries::threads::thread(&mut conn, thread)
            .await?
            .ok_or_else(|| EngineError::unreadable("a recovering thread is missing"))?;
        drop(conn);
        let directory = crate::sessions::absolute_directory(root, &row.workspace_dir);
        let mut artifacts = Vec::new();
        if !recipe.restored.contains_key("workspace")
            && (!recipe.missing.is_empty() || !recipe.listing_available)
        {
            let text = if recipe.listing_available {
                recipe.missing.join("\n") + "\n"
            } else {
                "Workspace listing unavailable; unlisted files could not be recovered.\n".into()
            };
            artifacts.push((
                "handoff/missing-files.txt".into(),
                Content::Bytes(text.into_bytes()),
            ));
        }
        if !recipe.restored.contains_key("home") && row.arrangement.as_deref() != Some("separated")
        {
            let kept = history(job, thread).await?;
            artifacts.push((
                crate::fork_history::handoff_path(thread),
                Content::Stored(kept.blob, kept.size),
            ));
            job.remember(&json!({format!("recovery_notice_{thread}"):kept.notice}))
                .await?;
        }
        materialize_artifacts(
            job,
            machine,
            recipe.environment,
            &format!("{}/.orch", directory.trim_end_matches('/')),
            generation,
            thread,
            artifacts,
        )
        .await?;
    }
    job.remember(&json!({"recovery_files_done":true})).await?;
    Ok(Done::Finished)
}

/// One thread's immutable recorded Carry supplier, retained by the environment job.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct HistoryArtifact {
    /// Where the transcript is kept in the store.
    pub(crate) blob: Hash,
    /// How large it is.
    pub(crate) size: i64,
    /// The notice that points the new agent at it.
    pub(crate) notice: String,
}

fn histories(plan: &Json) -> Result<BTreeMap<Id, HistoryArtifact>, EngineError> {
    plan.get("recovery_histories")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map(Option::unwrap_or_default)
        .map_err(Into::into)
}

/// Captures each thread once with the switch's renderer and Carry budget, without a model call. The conversation is written
/// to a file a page of turns at a time, from one snapshot of the history, and put in the store from the file: the
/// artifact is where it is kept and how large it is, with the notice that points at it. Nothing of the conversation is held
/// but the page being written.
pub(crate) async fn history(
    job: &crate::jobs::runner::Job,
    thread: Id,
) -> Result<HistoryArtifact, crate::jobs::runner::Stop> {
    if let Some(artifact) = histories(&job.plan().await?)?.get(&thread) {
        return Ok(artifact.clone());
    }
    let engine = job.engine();
    let scratch = crate::fork_history::cut::Scratch::new(engine.scratch_dir().join(format!(
        "recovery-{}-{}.md",
        job.id().simple(),
        thread.simple()
    )));
    let mut tx = engine.db().begin().await?;
    let row = orch_db::queries::threads::lock(&mut tx, thread)
        .await?
        .ok_or_else(|| EngineError::unreadable("the recovering thread is missing"))?;
    let unconfirmed = orch_db::queries::thread_history_cuts::unconfirmed_inputs(
        &mut tx,
        thread,
        row.active_continuation_id,
        None,
    )
    .await?;
    // The thread is locked, so nothing of it changes; one snapshot opened now sees all of it, page after page.
    let mut reading = orch_db::queries::feeds::begin_reading(engine.db()).await?;
    let last = orch_db::queries::thread_history::last_turn(
        &mut reading,
        thread,
        row.active_continuation_id,
    )
    .await?;
    let redactor = crate::raw_events::live_redactor(&mut tx, &row, row.environment_id)
        .await?
        .ok_or_else(|| EngineError::unreadable("the recorded conversation cannot be redacted"))?;
    let (turns, mut size) = if last == 0 {
        let text = crate::written::read_conversation_view(
            &mut reading,
            thread,
            row.active_continuation_id,
            (None, None),
            crate::written::Detail::Full,
            None,
            None,
        )
        .await?
        .ok_or_else(|| EngineError::unreadable("the recorded conversation is unavailable"))?
        .text;
        let text = crate::raw_events::mask_text(&redactor, text);
        std::fs::write(scratch.path(), text.as_bytes())
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        (0, i64::try_from(text.len()).unwrap_or(i64::MAX))
    } else {
        let written = crate::summaries::indexed_transcript_file(
            &mut reading,
            &row,
            None,
            Some(last),
            scratch.path(),
        )
        .await?;
        (written.turns, written.size)
    };
    let settings = crate::resolved::load(&mut tx, thread).await?;
    let model = match (row.provider_id, settings.model) {
        (Some(provider), Some(model)) => {
            orch_db::queries::resources::thread_engine::model(&mut tx, provider, &model).await?
        }
        _ => None,
    };
    let budget = crate::fork_history::carry_budget(
        &mut tx,
        thread,
        model
            .as_ref()
            .and_then(|model| crate::fork_history::model_window(&model.parameters)),
    )
    .await?;
    let recent = crate::written::read_conversation_view(
        &mut reading,
        thread,
        row.active_continuation_id,
        if budget.is_some() {
            (None, None)
        } else {
            (Some(-(FALLBACK_TURNS as i64)), None)
        },
        if budget.is_some() {
            crate::written::Detail::Normal
        } else {
            crate::written::Detail::Brief
        },
        budget,
        None,
    )
    .await?
    .ok_or_else(|| EngineError::unreadable("the recorded conversation is unavailable"))?;
    let carried = crate::summaries::Carried {
        from: recent.from,
        to: recent.to,
        brief: budget.is_none(),
        text: recent.text,
    };
    let warning = "Delivery of an interrupted input was not confirmed before its machine was lost. The record describes an attempt; do not assume its work completed. Failed inputs remain in the thread for their author to retry.";
    if unconfirmed {
        // Keep index offsets intact; the warning follows the indexed recorded turns.
        use std::io::Write as _;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(scratch.path())
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        file.write_all(format!("\n\n{warning}").as_bytes())
            .map_err(|error| EngineError::unreadable(error.to_string()))?;
        size += i64::try_from(warning.len() + 2).unwrap_or(0);
    }
    let mut notice = crate::summaries::handoff_notice(
        thread,
        turns,
        usize::try_from(size).unwrap_or(usize::MAX),
        None,
        Some((&carried, Time::new(job.now()))),
    );
    if unconfirmed {
        notice = format!("{warning}\n{notice}");
    }
    let notice = crate::raw_events::mask_text(&redactor, notice);
    tx.commit().await.map_err(DbError::from)?;
    reading.commit().await.map_err(DbError::from)?;

    // One environment job owns several thread suppliers. Its private plan and blob pins
    // freeze them independently; a single-thread history cut keeps its original owner rule.
    let (blob, size) = crate::fork_history::cut::put_file(engine, scratch.path()).await?;
    let artifact = HistoryArtifact { blob, size, notice };
    let mut tx = engine.db().begin().await?;
    let current = job
        .lock_owned(&mut tx)
        .await?
        .ok_or(crate::jobs::runner::Stop::Lost)?;
    let mut saved = histories(&current.plan)?;
    let artifact = if let Some(old) = saved.get(&thread) {
        old.clone()
    } else {
        storage::register_blob(&mut tx, &artifact.blob, artifact.size, job.now()).await?;
        if !saved.values().any(|held| held.blob == artifact.blob) {
            storage::pin_blobs(&mut tx, &[artifact.blob]).await?;
        }
        saved.insert(thread, artifact.clone());
        jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"recovery_histories":saved}),
            job.now(),
        )
        .await?;
        artifact
    };
    tx.commit().await.map_err(DbError::from)?;
    Ok(artifact)
}

/// Releases an environment job's unique history pins in its terminal transaction.
pub(crate) async fn release_history_pins(
    conn: &mut Conn,
    job: Id,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), EngineError> {
    let Some(row) = jobs::lock(conn, job).await? else {
        return Ok(());
    };
    if row.is_open() || row.plan.get("recovery_history_pins_released") == Some(&json!(true)) {
        return Ok(());
    }
    let saved = histories(&row.plan)?;
    if saved.is_empty() {
        return Ok(());
    }
    let mut hashes = saved
        .values()
        .map(|artifact| artifact.blob)
        .collect::<Vec<_>>();
    hashes.sort();
    hashes.dedup();
    storage::unpin_blobs(conn, &hashes, at).await?;
    jobs::merge_plan(
        conn,
        job,
        &json!({"recovery_history_pins_released":true}),
        at,
    )
    .await?;
    Ok(())
}

/// The switch handoff uses three brief recent turns when the model's context window is unknown.
const FALLBACK_TURNS: usize = 3;
/// Metadata artifacts are ordinary text files with owner read/write and public read permissions.
const ARTIFACT_MODE: u32 = 0o644;

/// Prepares only local agents whose native home cannot travel with a live move.
pub(crate) async fn prepare_histories(
    job: &crate::jobs::runner::Job,
    environment: Id,
    machine: Id,
    root: &str,
    generation: i32,
    home: bool,
) -> Result<crate::jobs::runner::Done, crate::jobs::runner::Stop> {
    if home {
        return Ok(crate::jobs::runner::Done::Finished);
    }
    let ids = {
        let mut conn = job.engine().db().acquire().await?;
        envs::threads_of(&mut conn, environment).await?
    };
    for id in ids {
        let row = {
            let mut conn = job.engine().db().acquire().await?;
            orch_db::queries::threads::thread(&mut conn, id).await?
        }
        .ok_or_else(|| EngineError::unreadable("the moved thread is missing"))?;
        if row.arrangement.as_deref() == Some("separated") {
            continue;
        }
        let kept = history(job, id).await?;
        let root = format!(
            "{}/.orch",
            crate::sessions::absolute_directory(root, &row.workspace_dir).trim_end_matches('/')
        );
        materialize_artifacts(
            job,
            machine,
            environment,
            &root,
            generation,
            id,
            vec![(
                crate::fork_history::handoff_path(id),
                Content::Stored(kept.blob, kept.size),
            )],
        )
        .await?;
        job.remember(&json!({format!("recovery_notice_{id}"):kept.notice}))
            .await?;
    }
    Ok(crate::jobs::runner::Done::Finished)
}

/// What a file written beside a recovered thread holds: bytes to put in the store, or content that is in it already.
pub(crate) enum Content {
    /// A small text, put in the store when the manifest is made.
    Bytes(Vec<u8>),
    /// Content already in the store and held by the job (its hash and size): a transcript, which is never held.
    Stored(Hash, i64),
}

pub(crate) async fn materialize_artifacts(
    job: &crate::jobs::runner::Job,
    machine: Id,
    environment: Id,
    root: &str,
    generation: i32,
    thread: Id,
    files: Vec<(String, Content)>,
) -> Result<(), crate::jobs::runner::Stop> {
    if files.is_empty() {
        return Ok(());
    }
    let mut conn = job.engine().db().acquire().await?;
    let mut entries = Vec::new();
    for (path, content) in files {
        let (hash, size) = match content {
            Content::Bytes(bytes) => {
                let size = bytes.len() as i64;
                (
                    job.engine().put_manifest(&mut conn, bytes).await?.hash,
                    size,
                )
            }
            Content::Stored(hash, size) => (hash, size),
        };
        entries.push(manifest::Entry {
            path,
            kind: EntryKind::File,
            size,
            mtime_ns: job.now().timestamp_nanos_opt().unwrap_or(0),
            mode: ARTIFACT_MODE,
            hash: Some(hash),
            link: None,
        });
    }
    let written = manifest::build(
        &manifest::Header {
            v: manifest::MANIFEST_VERSION,
            root: root.into(),
            generated_at: Time::new(job.now()),
            generation: i64::from(generation),
            case_sensitive: true,
        },
        entries,
    )
    .map_err(EngineError::unreadable)?;
    let manifest = job
        .engine()
        .put_manifest(&mut conn, written.bytes)
        .await?
        .hash;
    drop(conn);
    job.command(
        Outgoing::new(
            ids::derived(
                "recovery.artifacts",
                &[job.id().as_bytes(), thread.as_bytes(), manifest.as_bytes()],
            ),
            machine,
            SerialKey::Environment(environment),
            &orch_link_contract::commands::FsMaterialize {
                work: None,
                environment_id: environment,
                job_id: Some(job.id()),
                manifest_blob: manifest,
                target_root: root.into(),
                purpose: orch_link_contract::commands::MaterializePurpose::Delivery,
            },
        )
        .map_err(EngineError::from)?,
    )
    .await?;
    Ok(())
}

/// Renews native sessions only when their home traveled; every other local agent receives its recorded Carry history.
pub(crate) async fn commit_threads(
    job: &crate::jobs::runner::Job,
    conn: &mut Conn,
    environment: Id,
    generation: i32,
    by: Id,
    home: bool,
    recreated: bool,
) -> Result<(), crate::jobs::runner::Stop> {
    let ids = envs::threads_of(conn, environment).await?;
    let plan = jobs::job(conn, job.id())
        .await?
        .ok_or_else(|| EngineError::unreadable("the recovering job is missing"))?
        .plan;
    let mut unit = crate::unit::Unit::new(conn, job.now());
    for id in ids {
        let row = orch_db::queries::threads::thread(unit.conn, id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the recovering thread is missing"))?;
        let from_record = !home && row.arrangement.as_deref() != Some("separated");
        crate::jobs::recreate::renew_segment(
            &mut unit,
            id,
            Some(generation),
            if recreated {
                "environment_recreated"
            } else {
                "environment_moved"
            },
            by,
            false,
        )
        .await?;
        let seq = unit.stamp(id).await?;
        if from_record {
            sqlx::query("UPDATE segments SET native_session_ref=NULL WHERE id=(SELECT current_segment_id FROM threads WHERE node_id=$1)").bind(id).execute(&mut *unit.conn).await.map_err(DbError::from)?;
            job.engine().forget_open_sessions(id);
        }
        let key = format!("recovery:{}", job.id());
        if !orch_db::queries::threads::notices::contains(unit.conn, id, &key).await? {
            let history = plan
                .get(format!("recovery_notice_{id}"))
                .and_then(Json::as_str)
                .unwrap_or_default();
            if from_record && history.is_empty() {
                return Err(crate::jobs::runner::Failure::new(
                    "history_missing",
                    "The recorded Carry handoff was not prepared",
                )
                .into());
            }
            let text = format!(
                "The environment was {} on another machine. Your directory is {}. Earlier processes are not running here.{}\n{}",
                if recreated { "re-created" } else { "moved" },
                row.workspace_dir,
                if plan
                    .pointer("/recovery/missing")
                    .and_then(Json::as_array)
                    .is_some_and(|paths| !paths.is_empty())
                {
                    " Some files could not be recovered; see .orch/handoff/missing-files.txt."
                } else {
                    ""
                },
                history
            );
            orch_db::queries::threads::notices::append(
                unit.conn,
                id,
                &json!({"id":key,"kind":"handoff","text":text}),
            )
            .await?;
            orch_db::queries::threads::set_seq_state(unit.conn, id, seq).await?;
        }
        orch_db::queries::threads::set_state(
            unit.conn,
            id,
            &["blocked", "idle"],
            &orch_db::queries::threads::StateChange::plain("idle"),
            seq,
        )
        .await?;
        unit.wake(id);
    }
    unit.finish().await?;
    Ok(())
}
