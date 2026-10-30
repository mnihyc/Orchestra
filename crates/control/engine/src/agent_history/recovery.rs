//! A missing native session resumes from a frozen conversation through the switch handoff helpers.

use super::*;
use crate::fork_history;
use crate::manifest::{Entry, EntryKind, Header, MANIFEST_VERSION, build};
use crate::sessions::{SessionImport, StartSnapshot};
use crate::summaries::handoff_notice;
use crate::unit::Unit;
use orch_db::queries::{agent_history, thread_history_cuts as cuts};
use orch_link_contract::commands::{
    FsMaterialize, FsMaterializeResult, MaterializePurpose, SessionMode, SessionOpenHistory,
};
use orch_types::{Hash, Json, Time};
use serde_json::json;

/// Summary's existing fallback is three brief turns when a producer or window is unavailable.
const BRIEF_TURNS: usize = 3;

impl Engine {
    /// The exact old lane is closed before its replacement, including when only the files were lost.
    pub(crate) async fn close_missing_agent_history(
        &self,
        snapshot: &StartSnapshot,
    ) -> Result<Outcome, EngineError> {
        let command = Outgoing::new(
            crate::ids::derived(
                "history.close",
                &[snapshot.segment.id.as_bytes(), snapshot.turn.id.as_bytes()],
            ),
            snapshot.key.machine,
            SerialKey::Thread(snapshot.thread.node_id),
            &orch_link_contract::continuations::SessionCloseFenced {
                thread_id: snapshot.thread.node_id,
                segment_id: snapshot.segment.id,
            },
        )?;
        Ok(crate::commands::run(self.inner.commands.as_ref(), command).await?)
    }
    /// The accepted native import or delivered handoff records recovery and releases temporary pins.
    pub(crate) async fn complete_agent_history_recovery(
        &self,
        unit: &mut Unit<'_>,
        snapshot: &StartSnapshot,
        recipient: Id,
        delivered: bool,
    ) -> Result<(), EngineError> {
        let recipe: Option<Json> = sqlx::query_scalar(
            "SELECT agent_history_recovery FROM segments WHERE id=$1 AND thread_id=$2",
        )
        .bind(recipient)
        .bind(snapshot.thread.node_id)
        .fetch_one(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
        let Some(recipe) = recipe.filter(|recipe| recipe["done"] != true) else {
            return Ok(());
        };
        let cut_id: Option<Id> = serde_json::from_value(recipe["cut"].clone())?;
        let source_id: Id = serde_json::from_value(recipe["source_segment"].clone())?;
        let source = agent_history::get(unit.conn, source_id).await?;
        let cut = match cut_id {
            Some(id) => cuts::cut(unit.conn, id).await?,
            None => None,
        };
        use orch_api_contract::threads::AgentHistoryRecovery as Mode;
        let mode = if recipe["history"] == "none" {
            Mode::None
        } else if !cut
            .as_ref()
            .ok_or_else(|| EngineError::unreadable("The recovery history is unavailable."))?
            .artifacts["recovery_plan"]["native"]
            .is_null()
        {
            Mode::Native
        } else if cut
            .as_ref()
            .is_some_and(|cut| cut.artifacts["recovery_plan"]["summary"] == true)
        {
            Mode::Summary
        } else if cut
            .as_ref()
            .is_some_and(|cut| cut.artifacts["recovery_plan"]["brief"] == true)
        {
            Mode::Brief
        } else if cut
            .as_ref()
            .is_some_and(|cut| cut.artifacts["recovery_plan"]["carried"] != true)
        {
            Mode::File
        } else {
            Mode::Carry
        };
        if !delivered && !matches!(mode, Mode::Native | Mode::None) {
            return Ok(());
        }
        Self::agent_history_marker(
            unit,
            snapshot.thread.node_id,
            snapshot.segment.id,
            source.as_ref(),
            source_id,
            &snapshot.descriptor.harness.name,
            Some(mode),
            &format!("history-recovered:{recipient}"),
        )
        .await?;
        if let Some(id) = cut_id {
            cuts::release(unit.conn, id).await?;
        }
        sqlx::query("UPDATE segments SET agent_history_recovery=jsonb_set(agent_history_recovery,'{done}','true') WHERE id=$1")
            .bind(recipient).execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "one history record names the kept native session and its observed outcome"
    )]
    pub(super) async fn agent_history_marker(
        unit: &mut Unit<'_>,
        thread: Id,
        segment: Id,
        source: Option<&rows::History>,
        source_id: Id,
        program: &str,
        recovered: Option<orch_api_contract::threads::AgentHistoryRecovery>,
        key: &str,
    ) -> Result<(), EngineError> {
        let by = if let Some(by) = source.and_then(|source| source.removed_by) {
            Some(serde_json::from_value(
                crate::views::user_ref_json(unit.conn, by).await?,
            )?)
        } else {
            None
        };
        let payload = orch_api_contract::threads::MarkerPayload::AgentHistory(
            orch_api_contract::threads::MarkerAgentHistory {
                segment_id: source_id,
                program: program.to_owned(),
                bytes: source.and_then(|source| source.bytes),
                removed_at: source.and_then(|source| source.removed_at).map(Time::new),
                by,
                recovered,
            },
        );
        crate::ingest::add_marker(
            unit,
            thread,
            segment,
            None,
            &serde_json::to_value(payload)?,
            key,
        )
        .await
    }

    /// A confirmed missing cache is discovered before any native resume is attempted.
    pub(crate) async fn agent_history_was_removed(&self, segment: Id) -> Result<bool, EngineError> {
        Ok(
            agent_history::get(&mut *self.db().acquire().await?, segment)
                .await?
                .is_some_and(|history| history.removed_at.is_some()),
        )
    }

    /// Frozen history is materialized before session.open; paid summaries are never guessed again on retry.
    pub(crate) async fn recover_agent_history(
        &self,
        snapshot: &mut StartSnapshot,
    ) -> Result<Option<(SessionMode, SessionImport)>, EngineError> {
        let continued = snapshot.segment.native_session_ref.is_some();
        let recipe: Option<Json> =
            sqlx::query_scalar("SELECT agent_history_recovery FROM segments WHERE id=$1")
                .bind(snapshot.segment.id)
                .fetch_one(&mut *self.db().acquire().await?)
                .await
                .map_err(orch_db::DbError::from)?;
        let Some(recipe) = recipe else {
            return Ok(None);
        };
        if recipe["done"] == true {
            return Ok(None);
        }
        if snapshot.thread.side_of.is_some() {
            crate::side_threads::history::authorize_recovery(
                self,
                &mut *self.db().acquire().await?,
                &snapshot.thread,
                &snapshot.segment,
                self.inner.clock.now(),
            )
            .await?;
        }
        if recipe["history"] == "none" {
            return Ok((!continued).then_some((SessionMode::New, SessionImport::default())));
        }
        if snapshot.thread.side_reading.as_deref() == Some("no_tools")
            && recipe["history"] == "summary"
        {
            return Err(EngineError::unreadable(
                "This side needs a native import to recover its conversation.",
            ));
        }
        let cut_id: Id = serde_json::from_value(recipe["cut"].clone())?;
        let mut cut = cuts::cut(&mut *self.db().acquire().await?, cut_id)
            .await?
            .filter(|cut| {
                cut.thread_id == snapshot.thread.node_id
                    && cut.owner_thread_id == Some(snapshot.thread.node_id)
            })
            .ok_or_else(|| {
                EngineError::unreadable("The retained recovery conversation is unavailable.")
            })?;
        if cut.artifacts.get("recovery_transcript").is_none() {
            // Written to a file a page of turns at a time and put in the store from it.
            fork_history::cut::transcript_file(self, &cut, "recovery_transcript").await?;
            cut = cuts::cut(&mut *self.db().acquire().await?, cut_id)
                .await?
                .ok_or_else(|| {
                    EngineError::unreadable("The retained recovery conversation disappeared.")
                })?;
        }
        let transcript_meta = cut.artifacts["recovery_transcript"].clone();
        let transcript_turns = i64::from(cut.last_turn.max(0) - cut.first_turn.max(1) + 1).max(0);
        let transcript_bytes = transcript_meta["size"]
            .as_i64()
            .filter(|size| *size >= 0)
            .ok_or_else(|| {
                EngineError::unreadable("The retained recovery transcript has no size.")
            })?;
        if cut.artifacts.get("recovery_plan").is_none() {
            let wants_summary = recipe["history"] == "summary";
            let mut summary = None;
            if wants_summary && cut.artifacts.get("recovery_summary_attempted").is_none() {
                let attempted = sqlx::query("UPDATE thread_history_cuts SET artifacts=jsonb_set(artifacts,'{recovery_summary_attempted}','true')
                    WHERE id=$1 AND NOT (artifacts ? 'recovery_summary_attempted')")
                    .bind(cut.id).execute(&mut *self.db().acquire().await?).await.map_err(orch_db::DbError::from)?.rows_affected() == 1;
                if attempted {
                    let made = async {
                        let choice = crate::summaries::capture_handoff_choice(
                            &mut *self.db().acquire().await?,
                            cut.thread_id,
                            cut.head["name"].as_str(),
                        )
                        .await?;
                        let summary_budget =
                            crate::summaries::handoff_budget(self, cut.thread_id, &choice).await?;
                        let previous =
                            fork_history::cut::summary_input(self, &cut, &summary_budget).await?;
                        crate::summaries::handoff_captured(
                            self,
                            cut.thread_id,
                            &previous,
                            cut.id,
                            &choice,
                        )
                        .await
                    }
                    .await;
                    match made {
                        Ok(text) => {
                            cuts::keep_artifact(
                                &mut *self.db().acquire().await?,
                                cut.id,
                                "recovery_summary",
                                &json!({"text":text,"at":Time::new(self.inner.clock.now())}),
                                &[],
                            )
                            .await?;
                        }
                        Err(error) => {
                            tracing::info!(%error,"the recovery uses brief recent turns because its summary is unavailable");
                        }
                    }
                }
                cut = cuts::cut(&mut *self.db().acquire().await?, cut_id)
                    .await?
                    .ok_or_else(|| {
                        EngineError::unreadable("The retained recovery conversation disappeared.")
                    })?;
            }
            if let Some(saved) = cut.artifacts.get("recovery_summary") {
                summary = Some((
                    saved["text"]
                        .as_str()
                        .ok_or_else(|| EngineError::unreadable("The retained summary is invalid."))?
                        .to_owned(),
                    serde_json::from_value::<Time>(saved["at"].clone())?,
                ));
            }
            let window = snapshot
                .model
                .as_ref()
                .and_then(|model| fork_history::model_window(&model.parameters));
            let budget = if wants_summary {
                None
            } else {
                fork_history::carry_budget(
                    &mut *self.db().acquire().await?,
                    snapshot.thread.node_id,
                    window,
                )
                .await?
            };
            let recent = if summary.is_some() {
                None
            } else {
                fork_history::cut::recent(
                    &mut *self.db().acquire().await?,
                    &cut,
                    if budget.is_some() {
                        crate::written::Detail::Normal
                    } else {
                        crate::written::Detail::Brief
                    },
                    budget,
                    budget.is_none().then_some(BRIEF_TURNS),
                )
                .await?
            };
            let mut native = None;
            if !wants_summary
                && snapshot.descriptor.capabilities.import_neutral
                && let (Some(budget), Some(recent)) = (budget, &recent)
            {
                // The turns the notice names, told as items if all of them fit the share of the window: they are read
                // from the end of the cut a page at a time, and the reading stops where the share is used.
                let newest = fork_history::newest(
                    &mut *self.db().acquire().await?,
                    crate::history_pages::Source::Cut(&cut),
                    (recent.from, recent.to),
                    i64::from(cut.last_turn),
                    budget,
                )
                .await?;
                let text = if newest.complete {
                    fork_history::neutral_lines(&newest.selected)
                } else {
                    String::new()
                };
                let whole_side = snapshot.thread.side_reading.as_deref() != Some("no_tools")
                    || recent.from == i64::from(cut.first_turn)
                        && recent.to == i64::from(cut.last_turn);
                if text.len() <= budget && !text.is_empty() && whole_side {
                    let (blob, size) = fork_history::cut::keep_bytes(
                        self,
                        cut.id,
                        "recovery_neutral",
                        text.into_bytes(),
                    )
                    .await?;
                    native = Some(json!({"blob":blob,"size":size}));
                }
            }
            let notice = handoff_notice(
                cut.thread_id,
                transcript_turns,
                usize::try_from(transcript_bytes).unwrap_or(usize::MAX),
                summary.as_ref().map(|(text, at)| (text.as_str(), *at)),
                recent
                    .as_ref()
                    .map(|recent| (recent, Time::new(cut.captured_at))),
            );
            let plan = json!({"native":native,"notice":notice,"summary":summary.is_some(),"brief":recent.as_ref().is_some_and(|recent|recent.brief),"carried":recent.is_some(),"summary_skipped":wants_summary && summary.is_none()});
            cuts::keep_artifact(
                &mut *self.db().acquire().await?,
                cut.id,
                "recovery_plan",
                &plan,
                &[],
            )
            .await?;
            cut = cuts::cut(&mut *self.db().acquire().await?, cut_id)
                .await?
                .ok_or_else(|| {
                    EngineError::unreadable("The retained recovery conversation disappeared.")
                })?;
        }
        if cut.artifacts["recovery_plan"]["native"].is_null()
            && snapshot.thread.side_reading.as_deref() == Some("no_tools")
        {
            return Err(EngineError::unreadable(
                "This side needs a native import to recover its conversation.",
            ));
        }
        let plan = &cut.artifacts["recovery_plan"];
        if !plan["native"].is_null() {
            if continued {
                return Ok(None);
            }
            return Ok(Some((
                SessionMode::ImportNeutral,
                SessionImport {
                    history: Some(SessionOpenHistory {
                        blob: serde_json::from_value(plan["native"]["blob"].clone())?,
                        size: plan["native"]["size"].as_i64().ok_or_else(|| {
                            EngineError::unreadable("The retained native history has no size.")
                        })?,
                    }),
                    ..SessionImport::default()
                },
            )));
        }
        if snapshot.thread.side_of.is_some() {
            crate::side_threads::history::authorize_recovery(
                self,
                &mut *self.db().acquire().await?,
                &snapshot.thread,
                &snapshot.segment,
                self.inner.clock.now(),
            )
            .await?;
        }
        self.materialize_agent_history(snapshot, &cut, &transcript_meta)
            .await?;
        let notice = plan["notice"].as_str().ok_or_else(|| {
            EngineError::unreadable("The recovery handoff notice is unavailable.")
        })?;
        let mut tx = self.db().begin().await?;
        let finished = {
            let mut unit = Unit::new(&mut tx, self.inner.clock.now());
            let row = threads::lock(unit.conn, snapshot.thread.node_id)
                .await?
                .ok_or_else(|| EngineError::unreadable("The conversation is unavailable."))?;
            let id = format!("agent-history:{}", snapshot.segment.id);
            if threads::notices::append(
                unit.conn,
                row.node_id,
                &json!({"id":id,"kind":"handoff","text":notice}),
            )
            .await?
            {
                let seq = unit.stamp(row.node_id).await?;
                threads::set_seq_state(unit.conn, row.node_id, seq).await?;
            }
            snapshot.thread.pending_notices = json!(
                threads::notices::for_segment(unit.conn, row.node_id, snapshot.segment.id)
                    .await?
                    .into_iter()
                    .map(|notice| notice.notice)
                    .collect::<Vec<_>>()
            );
            unit.finish().await?
        };
        self.commit(tx, finished).await?;
        Ok((!continued).then_some((SessionMode::New, SessionImport::default())))
    }

    async fn materialize_agent_history(
        &self,
        snapshot: &StartSnapshot,
        cut: &cuts::Cut,
        meta: &Json,
    ) -> Result<(), EngineError> {
        let root = format!("{}/.orch", snapshot.directory().trim_end_matches('/'));
        let blob: Hash = serde_json::from_value(meta["blob"].clone())?;
        let size = meta["size"]
            .as_i64()
            .ok_or_else(|| EngineError::unreadable("The transcript size is unavailable."))?;
        let manifest_key = format!(
            "recovery_manifest:{}:{}:{}",
            snapshot.environment.id,
            snapshot.environment.generation,
            Hash::of(root.as_bytes())
        );
        let manifest = if let Some(meta) = cut.artifacts.get(&manifest_key) {
            fork_history::cut::read_bytes(self, meta).await?;
            serde_json::from_value(meta["blob"].clone())?
        } else {
            let written = build(
                &Header {
                    v: MANIFEST_VERSION,
                    root: root.clone(),
                    generated_at: Time::new(cut.captured_at),
                    generation: i64::from(snapshot.environment.generation),
                    case_sensitive: true,
                },
                vec![Entry {
                    path: fork_history::handoff_path(cut.thread_id),
                    kind: EntryKind::File,
                    size,
                    mtime_ns: cut.captured_at.timestamp_nanos_opt().unwrap_or(0),
                    mode: 0o644,
                    hash: Some(blob),
                    link: None,
                }],
            )
            .map_err(EngineError::unreadable)?;
            fork_history::cut::keep_bytes(self, cut.id, &manifest_key, written.bytes)
                .await?
                .0
        };
        let machine = snapshot.environment.machine_id.ok_or_else(|| {
            EngineError::unreadable("The thread has no machine for its transcript.")
        })?;
        let command = Outgoing::new(
            crate::ids::derived(
                "history.materialize",
                &[
                    snapshot.segment.id.as_bytes(),
                    snapshot.environment.id.as_bytes(),
                    machine.as_bytes(),
                    manifest.as_bytes(),
                ],
            ),
            machine,
            SerialKey::Environment(snapshot.environment.id),
            &FsMaterialize {
                work: None,
                environment_id: snapshot.environment.id,
                job_id: None,
                manifest_blob: manifest,
                target_root: root,
                purpose: MaterializePurpose::Delivery,
            },
        )?;
        cuts::hold_command(
            &mut *self.db().acquire().await?,
            cut.id,
            command.id,
            machine,
        )
        .await?;
        let command_id = command.id;
        match crate::commands::run(self.inner.commands.as_ref(), command).await? {
            Outcome::Succeeded(value) => {
                let result: FsMaterializeResult = serde_json::from_value(value)?;
                if result.files.saturating_add(result.skipped) < 1 {
                    return Err(EngineError::unreadable(
                        "The recovery transcript was not confirmed in the workspace.",
                    ));
                }
                cuts::settle_command(&mut *self.db().acquire().await?, cut.id, command_id).await?;
                Ok(())
            }
            _ => Err(EngineError::unreadable(
                "The recovery transcript was not confirmed in the workspace.",
            )),
        }
    }
}
