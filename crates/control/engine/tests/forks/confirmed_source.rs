//! Explicit synthetic native-record peer and actual guarded full-fork/side jobs.
//! This is domain/link/journal composition, not the pinned vendor-model proof.

use super::*;
use async_trait::async_trait;
use orch_agent_contract::{
    discovery::{RecordedEntry, RecordedItem},
    events::ItemContent,
};
use orch_db::queries::{storage, thread_history, thread_history_cuts as cuts, threads};
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing};
use orch_link_contract::commands::{AgentReadSessionResult, RecordRef};
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::{Notify, Semaphore};

#[path = "confirmed_source/review.rs"]
mod review;

struct SuccessGate {
    inner: Arc<dyn Commands>,
    source: Id,
    saved: Mutex<Option<Outgoing>>,
    waiting: std::sync::atomic::AtomicBool,
    reached: Notify,
    release: Semaphore,
}

#[async_trait]
impl Commands for SuccessGate {
    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        self.inner.observe(command).await
    }
    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        if command.kind == CommandKind::SessionOpenFenced
            && command.body["thread_id"] != json!(self.source)
        {
            let mut saved = self.saved.lock().unwrap();
            if saved.is_none() {
                *saved = Some(command.clone());
            }
        }
        self.inner.issue(command).await
    }
    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        let result = self.inner.wait(id).await?;
        let selected = self
            .saved
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|saved| saved.id == id);
        if selected && matches!(&result, Outcome::Succeeded(_)) {
            self.waiting.store(true, Ordering::Release);
            self.reached.notify_waiters();
            self.release
                .acquire()
                .await
                .map_err(|_| CommandsError::Stopping)?
                .forget();
        }
        Ok(result)
    }
    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        self.inner.retained_body(id).await
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::commands::RetainedCommand>, CommandsError> {
        self.inner.retained_command(id).await
    }
    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        self.inner.status(id).await
    }
    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.cancel(id).await
    }
    fn hears_behind(&self, machine: Id, stream: &Stream) -> bool {
        self.inner.hears_behind(machine, stream)
    }
}

#[derive(Clone, Default)]
struct NativeSession {
    turns: HashMap<Id, i64>,
    entries: Vec<RecordedEntry>,
    next: i64,
    opaque: bool,
    profile: Option<(Id, Id)>,
}

fn native_reference(entry: &RecordedEntry) -> &str {
    match entry {
        RecordedEntry::Said(words) => &words.native_ref,
        RecordedEntry::Item(item) => &item.native_ref,
    }
}

impl NativeSession {
    fn through(&self, through: i64) -> Self {
        Self {
            turns: self
                .turns
                .iter()
                .filter(|(_, ordinal)| **ordinal <= through)
                .map(|(id, ordinal)| (*id, *ordinal))
                .collect(),
            entries: self
                .entries
                .iter()
                .filter(|entry| entry.turn() <= through)
                .cloned()
                .collect(),
            next: through,
            opaque: self.opaque,
            profile: self.profile,
        }
    }
    fn bytes(&self) -> Vec<u8> {
        self.entries
            .iter()
            .map(|entry| serde_json::to_string(entry).unwrap() + "\n")
            .collect::<String>()
            .into_bytes()
    }
}

#[derive(Default)]
pub(super) struct NativePeer {
    sessions: HashMap<String, NativeSession>,
    threads: HashMap<Id, String>,
    archives: HashMap<Hash, NativeSession>,
    extra_next_fork: Option<RecordedEntry>,
}

impl NativePeer {
    fn initial(
        &self,
        open: &orch_link_contract::commands::SessionOpen,
    ) -> Result<NativeSession, CommandError> {
        match open.mode {
            SessionMode::Fork => {
                let parent = open
                    .native_session_ref
                    .as_ref()
                    .and_then(|native| self.sessions.get(native))
                    .ok_or_else(|| refusal("the actual native fork source is unavailable"))?;
                let through = i64::from(open.upto_turn_ordinal.ok_or_else(|| {
                    refusal("the declared peer requires an actual bounded native ordinal")
                })?);
                if parent.opaque
                    || through <= 0
                    || through > parent.next
                    || !parent.turns.values().any(|ordinal| *ordinal == through)
                {
                    return Err(CommandError::new(
                        ErrorCode::Unsupported,
                        "the actual native source has no such complete ordinal",
                    ));
                }
                Ok(parent.through(through))
            }
            SessionMode::ImportNative => open
                .archive
                .as_ref()
                .and_then(|archive| self.archives.get(&archive.blob))
                .cloned()
                .ok_or_else(|| {
                    refusal("the declared peer has no uploaded native archive with that identity")
                }),
            SessionMode::Resume | SessionMode::Adopt => Ok(open
                .native_session_ref
                .as_ref()
                .and_then(|native| self.sessions.get(native))
                .cloned()
                .unwrap_or_else(|| NativeSession {
                    opaque: true,
                    ..Default::default()
                })),
            SessionMode::ImportNeutral => Ok(NativeSession {
                opaque: true,
                ..Default::default()
            }),
            SessionMode::New => Ok(NativeSession::default()),
        }
    }
    fn reply(
        &self,
        native: &str,
        provider: Id,
        agent: Id,
    ) -> Result<(Vec<u8>, AgentReadSessionResult), CommandError> {
        let source = self
            .sessions
            .get(native)
            .filter(|source| !source.opaque && source.profile == Some((provider, agent)))
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::Unsupported,
                    "the declared peer has no exact record for that native source",
                )
            })?;
        let mut turns = BTreeMap::<i64, Vec<RecordedEntry>>::new();
        for entry in &source.entries {
            turns.entry(entry.turn()).or_default().push(entry.clone());
        }
        let turns = turns.into_values().collect::<Vec<_>>();
        let (text, kept, count, left_out) = orch_agent_contract::discovery::record_lines(
            &turns,
            orch_agent_contract::discovery::RECORD_MAX_BYTES,
        )
        .map_err(|_| refusal("the declared native record could not be encoded"))?;
        let skipped = usize::try_from(left_out).unwrap();
        let report = orch_agent_contract::discovery::record_report(&turns[skipped..], left_out);
        let bytes = text.into_bytes();
        let hash = Hash::of(&bytes);
        let size = i64::try_from(bytes.len()).unwrap();
        Ok((
            bytes,
            AgentReadSessionResult {
                model: Some(MODEL.into()),
                effort: Some("low".into()),
                turns: kept,
                entries: count,
                record: Some(RecordRef { blob: hash, size }),
                report,
            },
        ))
    }
}

impl Rig {
    /// Both real wire shapes reach one declared peer. The wrapper retains the
    /// actual received body and copies only the requested native range; a new
    /// controller thread/segment is never mistaken for that native ordinal.
    pub(super) fn on_open(
        &self,
        script: impl Fn(
            &orch_link_contract::commands::SessionOpen,
        ) -> Result<SessionOpenResult, CommandError>
        + Send
        + Sync
        + 'static,
    ) {
        let script = Arc::new(script);
        for kind in [CommandKind::SessionOpen, CommandKind::SessionOpenFenced] {
            let (script, peer, ordered, refusals, machine) = (
                script.clone(),
                self.native.clone(),
                self.world.ordered.clone(),
                self.open_refusals.clone(),
                self.world.machine_id,
            );
            self.world.machine.handler.on(kind, move |command| {
                ordered.lock().unwrap().push((machine, command.clone()));
                let open = match command {
                    Command::SessionOpen(open) => open,
                    Command::SessionOpenFenced(open) => &open.session,
                    _ => return Err(refusal("not an actual ordinary/fenced native open")),
                };
                if !matches!(open.mode, SessionMode::Resume | SessionMode::Adopt)
                    && let Some(error) = refusals.lock().unwrap().pop_front()
                {
                    return Err(error);
                }
                let mut initial = peer.lock().unwrap().initial(open)?;
                let result = script(open)?;
                let provider = match &open.provider {
                    orch_link_contract::commands::SessionOpenProvider::Native { provider_id }
                    | orch_link_contract::commands::SessionOpenProvider::Forwarded {
                        provider_id,
                        ..
                    }
                    | orch_link_contract::commands::SessionOpenProvider::Shared {
                        provider_id,
                        ..
                    }
                    | orch_link_contract::commands::SessionOpenProvider::Handover {
                        provider_id,
                        ..
                    } => *provider_id,
                };
                initial.profile = Some((provider, open.agent_id));
                let mut peer = peer.lock().unwrap();
                if open.mode == SessionMode::Fork
                    && let Some(extra) = peer.extra_next_fork.take()
                {
                    initial.entries.push(extra);
                }
                peer.sessions
                    .insert(result.native_session_ref.clone(), initial);
                peer.threads
                    .insert(open.thread_id, result.native_session_ref.clone());
                Ok(if kind == CommandKind::SessionOpen {
                    CommandResult::SessionOpen(result)
                } else {
                    CommandResult::SessionOpenFenced(result)
                })
            });
        }
    }
    pub(super) fn refuse_preparations(&self, errors: Vec<CommandError>) {
        self.open_refusals.lock().unwrap().extend(errors);
    }

    pub(super) fn on_export(
        &self,
        script: impl Fn(
            &orch_link_contract::commands::SessionExport,
        ) -> Result<SessionExportResult, CommandError>
        + Send
        + Sync
        + 'static,
    ) {
        let (peer, kept) = (self.native.clone(), self.exports.clone());
        self.world
            .machine
            .handler
            .on(CommandKind::SessionExport, move |command| {
                kept.lock().unwrap().push(command.clone());
                let Command::SessionExport(export) = command else {
                    return Err(refusal("not a native export"));
                };
                let source = {
                    let peer = peer.lock().unwrap();
                    let native = peer
                        .threads
                        .get(&export.thread_id)
                        .ok_or_else(|| refusal("the exact source has no current native session"))?;
                    let source = peer
                        .sessions
                        .get(native)
                        .ok_or_else(|| refusal("the exact source record is unavailable"))?;
                    source.through(export.upto_turn_ordinal.map_or(source.next, i64::from))
                };
                let result = script(export)?;
                peer.lock().unwrap().archives.insert(result.blob, source);
                Ok(CommandResult::SessionExport(result))
            });
    }

    pub(super) fn install_peer(&self) {
        // The real connector always answers a probe. Recovery uses its ordered
        // completion as a barrier when cancellation won over an export result.
        let probe: orch_link_contract::session::MachineFacts = serde_json::from_value(json!({
            "hostname": "fork-peer", "os_version": "synthetic", "cpus": 1,
            "memory_bytes": 1024, "harnesses": [], "adapters": [], "plugins": [],
            "policy": null, "rates": {"up_bps": 0, "down_bps": 0, "round_trip_ms": 0},
            "clock_offset_ms": 0
        }))
        .unwrap();
        self.world
            .machine
            .handler
            .on(CommandKind::MachineProbe, move |_| {
                Ok(CommandResult::MachineProbe(probe.clone()))
            });
        self.on_open(|open| {
            Ok(SessionOpenResult {
                native_session_ref: if matches!(open.mode, SessionMode::Resume | SessionMode::Adopt)
                {
                    open.native_session_ref.clone().ok_or_else(|| {
                        refusal("a native resume needs its exact retained identity")
                    })?
                } else {
                    format!("native-{}", open.segment_id)
                },
                resumed: open.mode == SessionMode::Resume,
                import_report: None,
                instruction_sources: Vec::new(),
                approval_reviewer: None,
            })
        });
        let (peer, store, pool, clock) = (
            self.native.clone(),
            self.store.clone(),
            self.world.pool.clone(),
            self.world.clock.clone(),
        );
        self.world
            .machine
            .handler
            .on(CommandKind::AgentReadSession, move |command| {
                let Command::AgentReadSession(ask) = command else {
                    return Err(refusal("not a native record request"));
                };
                if !ask.write {
                    return Err(refusal("the native proof requires actual record bytes"));
                }
                let (bytes, reply) = peer.lock().unwrap().reply(
                    &ask.native_session_ref,
                    ask.provider_id,
                    ask.agent_id,
                )?;
                let record = reply
                    .record
                    .ok_or_else(|| refusal("the native record has no uploaded identity"))?;
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        store
                            .put(
                                &record.blob,
                                Some(record.size),
                                orch_blob_contract::store::stream_of(vec![bytes.into()]),
                            )
                            .await
                            .map_err(|_| refusal("record upload failed"))?;
                        storage::register_blob(
                            &mut *pool
                                .acquire()
                                .await
                                .map_err(|_| refusal("record metadata unavailable"))?,
                            &record.blob,
                            record.size,
                            clock.now(),
                        )
                        .await
                        .map_err(|_| refusal("record metadata failed"))?;
                        Ok::<_, CommandError>(())
                    })
                })?;
                Ok(CommandResult::AgentReadSession(reply))
            });
        let (peer, store, pool, clock) = (
            self.native.clone(),
            self.store.clone(),
            self.world.pool.clone(),
            self.world.clock.clone(),
        );
        self.on_export(move |export| {
            let bytes = {
                let peer = peer.lock().unwrap();
                let native = peer
                    .threads
                    .get(&export.thread_id)
                    .ok_or_else(|| refusal("the exact export source is unavailable"))?;
                let source = peer
                    .sessions
                    .get(native)
                    .ok_or_else(|| refusal("the native export record is unavailable"))?;
                source
                    .through(export.upto_turn_ordinal.map_or(source.next, i64::from))
                    .bytes()
            };
            let blob = Hash::of(&bytes);
            let size = i64::try_from(bytes.len()).unwrap();
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    store
                        .put(
                            &blob,
                            Some(size),
                            orch_blob_contract::store::stream_of(vec![bytes.into()]),
                        )
                        .await
                        .map_err(|_| refusal("archive upload failed"))?;
                    storage::register_blob(
                        &mut *pool
                            .acquire()
                            .await
                            .map_err(|_| refusal("archive metadata unavailable"))?,
                        &blob,
                        size,
                        clock.now(),
                    )
                    .await
                    .map_err(|_| refusal("archive metadata failed"))?;
                    Ok::<_, CommandError>(())
                })
            })?;
            Ok(SessionExportResult {
                format: "fake-v1".into(),
                blob,
                size,
            })
        });
    }

    /// A held handler has genuinely started, but its overriding/default script
    /// has not executed yet. Inspect its saved wire and disk journal directly.
    async fn held_open(&self, job: Id) -> orch_link_contract::commands::SessionOpen {
        let saved: orch_engine::commands::Outgoing =
            serde_json::from_value(self.job(job).await.plan["history_wire"]["open_0"].clone())
                .unwrap();
        let journal: orch_link::journal::Entry = serde_json::from_slice(
            &tokio::fs::read(
                self.world
                    .machine
                    .data_dir()
                    .join("journal")
                    .join(format!("{}.json", saved.id)),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(journal.cmd.id, saved.id);
        assert_eq!(journal.cmd.kind, CommandKind::SessionOpenFenced.as_str());
        assert!(journal.cmd.body == saved.body);
        assert_eq!(Some(journal.cmd.deadline), saved.deadline);
        let body: orch_link_contract::continuations::SessionOpenFenced =
            serde_json::from_value(journal.cmd.body).unwrap();
        *body.session
    }

    /// Append this peer's actual reported IDs in their observed order. Its own
    /// native counter uses real turn IDs, independently of controller ordinals.
    /// Saved model-input references/text supply the declared synthetic Said rows.
    pub(super) async fn record_native(&self, thread: Id) {
        let (segment, mut items, aliases, inputs) = {
            let mut conn = self.world.pool.acquire().await.unwrap();
            let row = threads::thread(&mut conn, thread).await.unwrap().unwrap();
            let Some(segment) = row.current_segment_id else {
                return;
            };
            let segment = threads::segment(&mut conn, segment).await.unwrap().unwrap();
            let mut items = thread_history::items_between(
                &mut conn,
                thread,
                row.active_continuation_id,
                0,
                i64::MAX,
                None,
            )
            .await
            .unwrap();
            if row.side_of.is_some()
                && let Some(cut) = cuts::cut(&mut conn, thread).await.unwrap()
            {
                let mut parent = cuts::items_between(&mut conn, cut.id, 0, cut.captured_seq, None)
                    .await
                    .unwrap();
                for item in &mut items {
                    item.seq_created += cut.captured_seq;
                }
                parent.extend(items);
                items = parent;
            }
            let ids: Vec<_> = items.iter().map(|item| item.id).collect();
            let aliases: Vec<(Id, String)> = sqlx::query_as(
                "SELECT item_id,native_ref FROM thread_native_item_aliases WHERE item_id=ANY($1)",
            )
            .bind(ids)
            .fetch_all(&mut *conn)
            .await
            .unwrap();
            let inputs:Vec<(Id,String,Json)>=sqlx::query_as("SELECT input_id,reference,blocks FROM model_inputs WHERE node_id=$1 AND purpose='input' AND input_id IS NOT NULL ORDER BY seq_created")
                .bind(thread).fetch_all(&mut *conn).await.unwrap();
            (
                segment,
                items,
                aliases.into_iter().collect::<HashMap<_, _>>(),
                inputs
                    .into_iter()
                    .map(|(input, reference, blocks)| (input, (reference, blocks)))
                    .collect::<HashMap<_, _>>(),
            )
        };
        let Some(native) = segment.native_session_ref.clone() else {
            return;
        };
        items.sort_by_key(|item| item.seq_created);
        let mut peer = self.native.lock().unwrap();
        let source = peer.sessions.entry(native).or_default();
        for item in items {
            let Some(turn) = item.turn_id else { continue };
            let words = if item.kind == "user_message" && item.native_ref.is_some() {
                item.payload["input_id"]
                    .as_str()
                    .and_then(|id| id.parse::<Id>().ok())
                    .and_then(|id| inputs.get(&id))
            } else {
                None
            };
            let reference = if let Some((reference, _)) = words {
                format!("said:{reference}")
            } else if matches!(
                item.kind.as_str(),
                "agent_message" | "tool_call" | "plan" | "reasoning" | "error"
            ) {
                let Some(reference) = aliases.get(&item.id).cloned().or(item.native_ref) else {
                    continue;
                };
                reference
            } else {
                continue;
            };
            if source
                .entries
                .iter()
                .any(|entry| native_reference(entry) == reference)
            {
                continue;
            }
            let ordinal = match source.turns.get(&turn) {
                Some(ordinal) => *ordinal,
                None => {
                    source.next += 1;
                    source.turns.insert(turn, source.next);
                    source.next
                }
            };
            let entry = if let Some((_, blocks)) = words {
                RecordedEntry::Said(orch_agent_contract::discovery::RecordedWords {
                    native_ref: reference,
                    turn: ordinal,
                    text: blocks
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|block| block["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                    left_out: 0,
                    at: None,
                })
            } else {
                RecordedEntry::Item(RecordedItem {
                    native_ref: reference,
                    turn: ordinal,
                    stream: item.stream,
                    parent_native_ref: None,
                    status: serde_json::from_value(json!(item.status)).unwrap(),
                    content: serde_json::from_value::<ItemContent>(
                        json!({"kind":item.kind,"payload":item.payload}),
                    )
                    .unwrap(),
                    at: None,
                })
            };
            source.entries.push(entry);
        }
    }
}

async fn accounted(rig: &Rig, thread: Id) {
    eventually(
        "the actual source native and terminal work is accounted",
        || async {
            !threads::turn_end_work_pending(&mut rig.world.pool.acquire().await.unwrap(), thread)
                .await
                .unwrap()
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_after_real_native_success_retains_the_original_command_child_and_authored_target_queue()
 {
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig
        .thread_with_history("Native success before metadata")
        .await;
    accounted(&rig, source).await;
    let slot = Arc::new(Mutex::new(None));
    let capture = slot.clone();
    rig.world
        .restart_api_using(move |inner| {
            let gate = Arc::new(SuccessGate {
                inner,
                source,
                saved: Mutex::new(None),
                waiting: std::sync::atomic::AtomicBool::new(false),
                reached: Notify::new(),
                release: Semaphore::new(0),
            });
            *capture.lock().unwrap() = Some(gate.clone());
            gate
        })
        .await;
    rig.world.engine().attach_blobs(rig.store.clone());
    let gate = slot.lock().unwrap().clone().unwrap();
    let original = threads::thread(&mut rig.world.pool.acquire().await.unwrap(), source)
        .await
        .unwrap()
        .unwrap();
    let ledger: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE thread_id=$1 ORDER BY id")
            .bind(source)
            .fetch_all(rig.world.pool.pool())
            .await
            .unwrap();
    let plan = rig.plan(source, &rig.fork()).await;
    let job = rig.confirm(source, &plan).await;
    let target = rig.new_thread_of(&job);
    tokio::time::timeout(common::PATIENCE, async {
        loop {
            let woke = gate.reached.notified();
            if gate.waiting.load(Ordering::Acquire) {
                break;
            }
            woke.await;
        }
    })
    .await
    .unwrap();
    let saved = gate.saved.lock().unwrap().clone().unwrap();
    let journal_path = rig
        .world
        .machine
        .data_dir()
        .join("journal")
        .join(format!("{}.json", saved.id));
    let before = tokio::fs::read(&journal_path).await.unwrap();
    let entry: orch_link::journal::Entry = serde_json::from_slice(&before).unwrap();
    let native: SessionOpenResult =
        serde_json::from_value(entry.result().unwrap().outcome.clone().unwrap()).unwrap();
    assert_eq!(entry.cmd.id, saved.id);
    assert!(entry.cmd.body == saved.body);
    assert_eq!(Some(entry.cmd.deadline), saved.deadline);
    assert!(
        rig.job(job.id).await.plan.get("opened").is_none(),
        "pause is genuinely before prepared metadata publication"
    );
    rig.say(target, "Retained exact revised input").await;
    let pending: Json =
        sqlx::query_scalar("SELECT to_jsonb(i) FROM thread_inputs i WHERE thread_id=$1")
            .bind(target)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    assert!(
        rig.world
            .engine()
            .cancel_job(job.id, rig.world.ada)
            .await
            .unwrap()
    );
    let ended = rig.wait_job(job.id, "cancelled").await;
    gate.release.add_permits(1);
    let cut = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), job.id)
        .await
        .unwrap()
        .unwrap();
    let result = &cut.artifacts[format!("native_result_{}", saved.id)];
    assert_eq!(
        result["result"]["native_session_ref"],
        json!(native.native_session_ref)
    );
    assert!(result["command"] == serde_json::to_value(&saved).unwrap());
    assert!(ended.plan["history_wire"]["open_0"] == serde_json::to_value(&saved).unwrap());
    assert_eq!(rig.world.machine.handler.runs_of(saved.id), 1);
    assert!(
        tokio::fs::read(&journal_path).await.unwrap() == before,
        "cancellation cannot replace the native Done receipt"
    );
    let after: Json =
        sqlx::query_scalar("SELECT to_jsonb(i) FROM thread_inputs i WHERE thread_id=$1")
            .bind(target)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    assert!(after == pending);
    let current = threads::thread(&mut rig.world.pool.acquire().await.unwrap(), source)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            current.active_continuation_id,
            current.execution_revision,
            current.current_segment_id
        ),
        (
            original.active_continuation_id,
            original.execution_revision,
            original.current_segment_id
        )
    );
    let after: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE thread_id=$1 ORDER BY id")
            .bind(source)
            .fetch_all(rig.world.pool.pool())
            .await
            .unwrap();
    assert!(after == ledger);
    assert!(rig.world.turn_starts(target).is_empty());
    assert_eq!(
        rig.opens()
            .iter()
            .filter(|open| open.thread_id == target && open.mode != SessionMode::Resume)
            .count(),
        1
    );
    assert!(rig.world.ordered.lock().unwrap().iter().any(|(_,command)|matches!(command,Command::SessionCloseFenced(close)
        if close.thread_id==target && close.segment_id==serde_json::from_value::<Id>(saved.body["session"]["segment_id"].clone()).unwrap())));
    // Test-owned wrappers retain a Process witness until these references go.
    drop(slot);
    drop(gate);
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unknown_extra_native_message_after_creation_fails_without_selecting_or_forking_another_child()
 {
    let rig = Rig::new(
        Can {
            fork_native: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig.thread_with_history("Complete native readback").await;
    accounted(&rig, source).await;
    let old = threads::thread(&mut rig.world.pool.acquire().await.unwrap(), source)
        .await
        .unwrap()
        .unwrap();
    let ledger: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE thread_id=$1 ORDER BY id")
            .bind(source)
            .fetch_all(rig.world.pool.pool())
            .await
            .unwrap();
    rig.native.lock().unwrap().extra_next_fork = Some(RecordedEntry::Said(
        orch_agent_contract::discovery::RecordedWords {
            native_ref: "said:unknown-native-interruption".into(),
            turn: 1,
            text: "An unexpected native user directive".into(),
            left_out: 0,
            at: None,
        },
    ));
    let plan = rig.plan(source, &rig.fork()).await;
    let job = rig.confirm(source, &plan).await;
    let ended = rig.wait_job(job.id, "failed").await;
    assert!(ended.plan.get("commit_done").is_none());
    let target = rig.new_thread_of(&job);
    let creations = rig
        .opens()
        .into_iter()
        .filter(|open| open.thread_id == target && open.mode == SessionMode::Fork)
        .collect::<Vec<_>>();
    assert_eq!(
        creations.len(),
        1,
        "a wrong successful child cannot cause another creation/fallback"
    );
    assert!(rig.world.turn_starts(target).is_empty());
    let current = threads::thread(&mut rig.world.pool.acquire().await.unwrap(), source)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            current.active_continuation_id,
            current.execution_revision,
            current.current_segment_id
        ),
        (
            old.active_continuation_id,
            old.execution_revision,
            old.current_segment_id
        )
    );
    let after: Vec<Json> =
        sqlx::query_scalar("SELECT to_jsonb(u) FROM usage_events u WHERE thread_id=$1 ORDER BY id")
            .bind(source)
            .fetch_all(rig.world.pool.pool())
            .await
            .unwrap();
    assert!(after == ledger);
    let wire: orch_engine::commands::Outgoing =
        serde_json::from_value(ended.plan["history_wire"]["open_0"].clone()).unwrap();
    assert_eq!(rig.world.machine.handler.runs_of(wire.id), 1);
    let journal: orch_link::journal::Entry = serde_json::from_slice(
        &tokio::fs::read(
            rig.world
                .machine
                .data_dir()
                .join("journal")
                .join(format!("{}.json", wire.id)),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let actual: SessionOpenResult =
        serde_json::from_value(journal.result().unwrap().outcome.clone().unwrap()).unwrap();
    let retained = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        retained.artifacts[format!("native_result_{}", wire.id)]["result"]["native_session_ref"],
        json!(actual.native_session_ref)
    );
    assert!(rig.world.ordered.lock().unwrap().iter().any(|(_,command)|matches!(command,
        Command::SessionCloseFenced(close) if close.thread_id==target && close.segment_id==creations[0].segment_id)));
    for kind in [CommandKind::FsRestore, CommandKind::FsMaterialize] {
        assert_eq!(
            rig.world.commands.issued_of(kind.as_str()),
            0,
            "native readback failure cannot undo files"
        );
    }
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn historical_unconfirmed_words_are_refused_instead_of_copied_out_of_their_original_queue() {
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig
        .thread_with_history("Unconfirmed input checkpoint")
        .await;
    accounted(&rig, source).await;
    let turn = threads::latest_turn(&mut rig.world.pool.acquire().await.unwrap(), source)
        .await
        .unwrap()
        .unwrap();
    // Explicit reconstruction of a legacy partial receipt, not a claim that a
    // new native turn ran or completed. The domain must refuse these DB facts.
    let mut tx = rig.world.pool.begin().await.unwrap();
    threads::lock(&mut tx, source).await.unwrap();
    let seq = orch_db::sequences::next(&mut tx, source)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "UPDATE thread_inputs SET state='pending',turn_id=NULL,delivered_at=NULL,queue_held=true,
        state_detail='history_delivery_unconfirmed',seq_updated=$2 WHERE id=$1",
    )
    .bind(turn.input_id)
    .bind(seq)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE threads SET queue_held=true,queue_held_by=$2,queue_held_at=$3,seq_state=$4 WHERE node_id=$1")
        .bind(source).bind(rig.world.ada).bind(rig.world.clock.now()).bind(seq).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let view = threads::thread(&mut rig.world.pool.acquire().await.unwrap(), source)
        .await
        .unwrap()
        .unwrap()
        .active_continuation_id;
    let mut conn = rig.world.pool.acquire().await.unwrap();
    assert!(
        cuts::unconfirmed_inputs(&mut conn, source, view, None)
            .await
            .unwrap(),
        "an unrestricted guard includes every owned turn with an unconfirmed input"
    );
    assert!(
        cuts::unconfirmed_inputs(&mut conn, source, view, Some((turn.ordinal, turn.ordinal)))
            .await
            .unwrap()
    );
    assert!(
        !cuts::unconfirmed_inputs(&mut conn, source, view, Some((1, 0)))
            .await
            .unwrap()
    );
    drop(conn);
    let before: Json = sqlx::query_scalar("SELECT to_jsonb(i) FROM thread_inputs i WHERE id=$1")
        .bind(turn.input_id)
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
    let native = CommandKind::ALL
        .iter()
        .map(|kind| rig.world.commands.issued_of(kind.as_str()))
        .sum::<usize>();
    let jobs = rig
        .world
        .count("SELECT count(*) FROM jobs WHERE node_id=$1", source)
        .await;
    let draft = rig
        .world
        .engine()
        .draft_fork(&rig.caller(), source, &rig.fork())
        .await
        .unwrap();
    assert!(
        draft
            .plan
            .blockers
            .iter()
            .any(|blocker| blocker.code == "history_source_unconfirmed")
    );
    let plan = rig
        .world
        .engine()
        .keep_fork_plan(&rig.caller(), draft)
        .await
        .unwrap();
    assert!(matches!(
        rig.world
            .engine()
            .confirm_fork(
                &rig.caller(),
                source,
                &ForkCreateBody {
                    notify: None,
                    preflight_id: plan.preflight_id,
                    options: None
                }
            )
            .await,
        Err(PlanError::Blocked(_))
    ));
    let after: Json = sqlx::query_scalar("SELECT to_jsonb(i) FROM thread_inputs i WHERE id=$1")
        .bind(turn.input_id)
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
    assert!(after == before);
    assert_eq!(
        rig.world
            .count("SELECT count(*) FROM jobs WHERE node_id=$1", source)
            .await,
        jobs
    );
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM thread_history_cuts WHERE thread_id=$1",
                source
            )
            .await,
        0
    );
    assert_eq!(
        CommandKind::ALL
            .iter()
            .map(|kind| rig.world.commands.issued_of(kind.as_str()))
            .sum::<usize>(),
        native
    );
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_preflight_cannot_silently_rebind_to_another_history_or_an_aba_execution() {
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig.thread_with_history("Captured preflight").await;
    accounted(&rig, source).await;
    let plan = rig.plan(source, &rig.fork()).await;
    let original = threads::thread(&mut rig.world.pool.acquire().await.unwrap(), source)
        .await
        .unwrap()
        .unwrap();
    // This is an explicitly synthetic selector checkpoint, not native version
    // activation. It isolates preflight CAS before any execution work is admitted.
    let mut tx = rig.world.pool.begin().await.unwrap();
    orch_db::queries::transaction_guard::lock_authority(&mut tx)
        .await
        .unwrap();
    threads::lock(&mut tx, source).await.unwrap();
    let input = threads::latest_turn(&mut tx, source)
        .await
        .unwrap()
        .unwrap();
    let position:i64=sqlx::query_scalar("SELECT seq_created FROM items WHERE thread_id=$1 AND turn_id=$2 AND kind='user_message' ORDER BY seq_created LIMIT 1")
        .bind(source).bind(input.id).fetch_one(&mut *tx).await.unwrap();
    let version = new_id();
    let seq = orch_db::sequences::next(&mut tx, source)
        .await
        .unwrap()
        .unwrap();
    orch_db::queries::thread_continuations::insert(
        &mut tx,
        &orch_db::queries::thread_continuations::NewContinuation {
            id: version,
            thread: source,
            parent: original.active_continuation_id,
            before_input: input.input_id,
            actor: rig.world.ada,
            at: rig.world.clock.now(),
            seq,
            queue: orch_db::queries::thread_continuations::QueueRow {
                held: original.queue_held,
                held_by: original.queue_held_by,
                held_at: original.queue_held_at,
                revision: original.queue_revision,
            },
        },
    )
    .await
    .unwrap();
    orch_db::queries::thread_continuations::freeze_prefix(
        &mut tx,
        source,
        original.active_continuation_id,
        version,
        input.ordinal,
        position,
        seq,
    )
    .await
    .unwrap();
    sqlx::query("UPDATE thread_continuations SET status='ready' WHERE id=$1")
        .bind(version)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE threads SET active_continuation_id=$2,execution_revision=execution_revision+1,current_segment_id=NULL,seq_state=$3 WHERE node_id=$1 AND current_turn_id IS NULL")
        .bind(source).bind(version).bind(seq).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    let jobs = rig
        .world
        .count("SELECT count(*) FROM jobs WHERE node_id=$1", source)
        .await;
    let inputs = rig
        .world
        .count(
            "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
            source,
        )
        .await;
    let native = CommandKind::ALL
        .iter()
        .map(|kind| rig.world.commands.issued_of(kind.as_str()))
        .sum::<usize>();
    let confirm = ForkCreateBody {
        notify: None,
        preflight_id: plan.preflight_id,
        options: None,
    };
    assert!(matches!(
        rig.world
            .engine()
            .confirm_fork(&rig.caller(), source, &confirm)
            .await,
        Err(PlanError::Expired)
    ));
    let mut tx = rig.world.pool.begin().await.unwrap();
    threads::lock(&mut tx, source).await.unwrap();
    let seq = orch_db::sequences::next(&mut tx, source)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE threads SET active_continuation_id=$2,execution_revision=execution_revision+1,current_segment_id=$3,seq_state=$4 WHERE node_id=$1")
        .bind(source).bind(original.active_continuation_id).bind(original.current_segment_id).bind(seq).execute(&mut *tx).await.unwrap();
    tx.commit().await.unwrap();
    assert!(matches!(
        rig.world
            .engine()
            .confirm_fork(&rig.caller(), source, &confirm)
            .await,
        Err(PlanError::Expired)
    ));
    assert_eq!(
        rig.world
            .count("SELECT count(*) FROM jobs WHERE node_id=$1", source)
            .await,
        jobs
    );
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id=$1",
                source
            )
            .await,
        inputs
    );
    assert_eq!(
        CommandKind::ALL
            .iter()
            .map(|kind| rig.world.commands.issued_of(kind.as_str()))
            .sum::<usize>(),
        native
    );
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM thread_history_cuts WHERE thread_id=$1",
                source
            )
            .await,
        0
    );
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_fork_uses_native_record_identity_when_controller_ordinals_are_sparse() {
    let rig = Rig::new(
        Can {
            fork_native: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig.thread_with_history("Sparse native source").await;
    rig.say(source, "Second source input").await;
    rig.world.run_turn(source, 1, "Second source answer").await;
    accounted(&rig, source).await;
    rig.record_native(source).await;
    // Reconstruct the legitimate sparse controller namespace (no native input,
    // new turn/session or history rewrite). The peer's accepted counter stays1/2.
    let before = rig.world.turn_starts(source).len();
    let source_opens = rig
        .opens()
        .iter()
        .filter(|open| open.thread_id == source)
        .count();
    sqlx::query("UPDATE turns SET ordinal=ordinal+100 WHERE thread_id=$1")
        .bind(source)
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    let plan = rig.plan(source, &rig.fork()).await;
    let job = rig.confirm(source, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    let target = rig.new_thread_of(&job);
    let open = rig
        .opens()
        .into_iter()
        .find(|open| open.thread_id == target && open.mode == SessionMode::Fork)
        .unwrap();
    assert_eq!(
        open.upto_turn_ordinal,
        Some(2),
        "real native ordinals come from the identified native record, never controller102"
    );
    assert_eq!(rig.world.turn_starts(source).len(), before);
    assert!(rig.world.turn_starts(target).is_empty());
    let visible = rig.world.sync(target).await;
    assert_eq!(
        visible
            .turns
            .iter()
            .map(|turn| turn.ordinal)
            .collect::<Vec<_>>(),
        [101, 102]
    );
    assert_eq!(
        common::said(&visible),
        [
            "user: Plan the work",
            "agent: The plan is ready",
            "user: Second source input",
            "agent: Second source answer"
        ]
    );
    let wrong: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM items i JOIN threads t ON t.node_id=i.thread_id
        WHERE i.thread_id=$1 AND i.continuation_id<>t.active_continuation_id",
    )
    .bind(target)
    .fetch_one(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(wrong, 0);
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM usage_events WHERE thread_id=$1",
                target
            )
            .await,
        0
    );
    rig.say(source, "Source input after the fork commits").await;
    rig.world
        .run_turn(source, 2, "Source answer after the fork commits")
        .await;
    accounted(&rig, source).await;
    assert_eq!(
        rig.opens()
            .iter()
            .filter(|open| open.thread_id == source)
            .count(),
        source_opens,
        "committing an ordinary Fork must not invalidate the original accepted native cache slot"
    );
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frozen_copied_rows_and_the_neutral_archive_share_one_cut_while_the_source_continues() {
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig.thread_with_history("Immutable shared cut").await;
    accounted(&rig, source).await;
    let request = rig.fork();
    let plan = rig.plan(source, &request).await;
    assert_eq!(plan.history.method, HistoryMethod::NeutralImport);
    rig.world
        .machine
        .handler
        .hold(CommandKind::SessionOpenFenced);
    let job = rig.confirm(source, &plan).await;
    let target = rig.new_thread_of(&job);
    rig.world
        .wait_began(CommandKind::SessionOpenFenced, 1)
        .await;
    let native = rig.held_open(job.id).await;
    assert_eq!(native.thread_id, target);
    let archive = native.history.unwrap();
    let before = rig.blob_text(&archive.blob).await;
    assert!(before.contains("The plan is ready"));
    // The real command is admitted/Started and still needs the uploaded
    // archive. Normal zero-grace collection must respect its cut-owned pin.
    let pinned = {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        let cut = cuts::cut(&mut conn, job.id).await.unwrap().unwrap();
        assert_eq!(cut.artifacts["neutral"]["blob"], json!(archive.blob));
        let before = storage::blob(&mut conn, &archive.blob)
            .await
            .unwrap()
            .unwrap();
        let value = cut.artifacts["neutral"].clone();
        assert!(
            cuts::keep_artifact(&mut conn, cut.id, "neutral", &value, &[archive.blob])
                .await
                .unwrap()
        );
        assert_eq!(
            storage::blob(&mut conn, &archive.blob)
                .await
                .unwrap()
                .unwrap()
                .pin_count,
            before.pin_count,
            "rejoining the same history artifact cannot allocate another blob pin"
        );
        let associations: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM thread_history_cut_blobs WHERE cut_id=$1 AND blob=$2",
        )
        .bind(job.id)
        .bind(archive.blob.as_bytes().as_slice())
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(associations, 1);
        assert!(before.pin_count >= 1);
        let idle = rig.world.clock.now() + chrono::Duration::seconds(1);
        assert!(
            !storage::unreferenced(&mut conn, idle, 1000)
                .await
                .unwrap()
                .iter()
                .any(|(hash, _)| *hash == archive.blob)
        );
        assert!(
            !storage::forget_blob(&mut conn, &archive.blob, idle)
                .await
                .unwrap()
        );
        before.pin_count
    };
    assert_eq!(
        rig.blob_text(&archive.blob).await,
        before,
        "the admitted native consumer still reads the actual stored archive bytes"
    );
    let source_opens = rig
        .opens()
        .iter()
        .filter(|open| open.thread_id == source)
        .count();
    {
        let mut tx = rig.world.pool.begin().await.unwrap();
        threads::lock(&mut tx, source).await.unwrap().unwrap();
        let seq = orch_db::sequences::next(&mut tx, source)
            .await
            .unwrap()
            .unwrap();
        sqlx::query("UPDATE items SET payload=jsonb_set(jsonb_set(payload,'{text}','\"Later edited display\"'),'{text_len}','20'),seq_updated=$2
            WHERE thread_id=$1 AND kind='agent_message'")
            .bind(source).bind(seq).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
    }
    rig.say(source, "New source suffix").await;
    rig.world.run_turn(source, 1, "New suffix answer").await;
    accounted(&rig, source).await;
    assert_eq!(
        rig.opens()
            .iter()
            .filter(|open| open.thread_id == source)
            .count(),
        source_opens,
        "ordinary Fork does not invalidate the original native cache slot"
    );
    assert_eq!(rig.blob_text(&archive.blob).await, before);
    rig.world.machine.handler.release();
    rig.wait_job(job.id, "succeeded").await;
    let fork = rig.world.sync(target).await;
    let said = common::said(&fork).join("\n");
    assert!(
        said.contains("The plan is ready")
            && !said.contains("Later edited display")
            && !said.contains("New source suffix")
            && !said.contains("New suffix answer")
    );
    assert_eq!(fork.turns.len(), 1);
    assert!(rig.world.turn_starts(target).is_empty());
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM thread_history_cuts WHERE id=$1",
                job.id
            )
            .await,
        0,
        "all consumers and independent copied payload pins publish before temporary cut retirement"
    );
    {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        assert_eq!(
            storage::blob(&mut conn, &archive.blob)
                .await
                .unwrap()
                .unwrap()
                .pin_count,
            pinned - 1,
            "successful native consumption releases exactly its cut association"
        );
        cuts::release(&mut conn, job.id).await.unwrap();
        assert_eq!(
            storage::blob(&mut conn, &archive.blob)
                .await
                .unwrap()
                .unwrap()
                .pin_count,
            pinned - 1,
            "repeated cut retirement must not unpin another owner"
        );
    }
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM usage_events WHERE thread_id=$1",
                target
            )
            .await,
        0
    );
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_target_keeps_its_actual_authored_queue_and_cannot_start_an_empty_replacement() {
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let source = rig.thread_with_history("Retained failed target").await;
    accounted(&rig, source).await;
    let plan = rig.plan(source, &rig.fork()).await;
    rig.world
        .machine
        .handler
        .hold(CommandKind::SessionOpenFenced);
    let job = rig.confirm(source, &plan).await;
    let target = rig.new_thread_of(&job);
    rig.world
        .wait_began(CommandKind::SessionOpenFenced, 1)
        .await;
    rig.say(target, "My exact pending words").await;
    let queued: Json =
        sqlx::query_scalar("SELECT to_jsonb(i) FROM thread_inputs i WHERE thread_id=$1")
            .bind(target)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    assert!(
        rig.world
            .engine()
            .cancel_job(job.id, rig.world.ada)
            .await
            .unwrap()
    );
    rig.world.machine.handler.release();
    rig.wait_job(job.id, "cancelled").await;
    let after: Json =
        sqlx::query_scalar("SELECT to_jsonb(i) FROM thread_inputs i WHERE thread_id=$1")
            .bind(target)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    assert!(
        after == queued,
        "unknown/cancelled native preparation cannot migrate or rewrite an authored pending InputRow"
    );
    assert!(
        !rig.world
            .scalar::<bool>(
                "SELECT deleted_at IS NOT NULL FROM nodes WHERE id=$1",
                target
            )
            .await
    );
    assert!(
        !rig.world
            .scalar::<bool>(
                "SELECT ready FROM thread_native_guards WHERE thread_id=$1",
                target
            )
            .await
    );
    rig.world.engine().preflight(target).await.unwrap();
    assert!(rig.world.turn_starts(target).is_empty());
    assert_eq!(
        rig.world
            .count("SELECT count(*) FROM turns WHERE thread_id=$1", target)
            .await,
        0
    );
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM usage_events WHERE thread_id=$1",
                target
            )
            .await,
        0
    );
    assert!(
        orch_engine::locks::held_by(&mut rig.world.pool.acquire().await.unwrap(), target)
            .await
            .unwrap()
            .is_some()
    );
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_full_fork_of_a_side_contains_its_captured_parent_and_keeps_newer_side_work() {
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("Captured parent").await;
    accounted(&rig, parent).await;
    let side = rig
        .world
        .engine()
        .start_side_thread(
            &rig.caller(),
            parent,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: "Side input".into(),
            }],
            "captured-side-prefix".into(),
            None,
        )
        .await
        .unwrap()
        .thread;
    rig.world.run_turn(side, 0, "Side answer").await;
    rig.world.wait_state(side, "idle").await;
    accounted(&rig, side).await;
    let prefix = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), side)
        .await
        .unwrap()
        .unwrap();
    let parent_positions: Vec<(Id, i64)> = sqlx::query_as(
        "SELECT item_id,seq_created FROM thread_history_cut_members($1) ORDER BY seq_created",
    )
    .bind(prefix.id)
    .fetch_all(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(prefix.last_turn, 1);
    assert!(prefix.captured_seq > 0);
    // Start every message before completing any. The declared peer's stream then
    // creates a dense run of prefix_seq+1 item positions: the shortest run
    // containing both p and p+prefix_seq. No item/turn/sequence row is invented.
    rig.say(side, "Longer side input").await;
    let longer = rig.world.turn_start(side, 1).await;
    rig.world
        .say(side, common::turn_began(longer.turn_id))
        .await;
    rig.world.wait_turn(longer.turn_id, "running").await;
    let segment = rig.world.segment_of(side).await;
    let stream = Stream::Session {
        thread_id: side,
        segment_id: segment,
    };
    let messages = usize::try_from(prefix.captured_seq)
        .unwrap()
        .checked_add(1)
        .unwrap();
    let references = (0..messages)
        .map(|n| format!("side-position-{n}"))
        .collect::<Vec<_>>();
    let answers = (0..messages)
        .map(|n| format!("Side ordered answer {n}"))
        .collect::<Vec<_>>();
    for reference in &references {
        rig.world
            .machine
            .publish(&stream, &common::message_began(longer.turn_id, reference))
            .await;
    }
    for (reference, answer) in references.iter().zip(&answers) {
        rig.world
            .machine
            .publish(&stream, &common::message_done(reference, answer))
            .await;
    }
    rig.world
        .machine
        .publish(
            &stream,
            &common::turn_ended(longer.turn_id, TurnOutcome::Completed),
        )
        .await;
    rig.world.wait_turn(longer.turn_id, "completed").await;
    rig.world.wait_state(side, "idle").await;
    accounted(&rig, side).await;
    let dense:Vec<i64>=sqlx::query_scalar("SELECT seq_created FROM items WHERE thread_id=$1 AND turn_id=$2 AND native_ref=ANY($3) ORDER BY seq_created")
        .bind(side).bind(longer.turn_id).bind(&references).fetch_all(rig.world.pool.pool()).await.unwrap();
    assert_eq!(dense.len(), messages);
    assert!(
        dense.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "the overlap premise uses actual consecutive stored item positions"
    );
    let overlap: Vec<(Id, Id, i64, i64)> = sqlx::query_as(
        "SELECT a.id,b.id,a.seq_created,b.seq_created FROM items a JOIN items b
        ON b.thread_id=a.thread_id AND b.turn_id=a.turn_id AND b.seq_created=a.seq_created+$3
        WHERE a.thread_id=$1 AND a.turn_id=$2 AND a.native_ref=ANY($4) AND b.native_ref=ANY($4)",
    )
    .bind(side)
    .bind(longer.turn_id)
    .bind(prefix.captured_seq)
    .bind(&references)
    .fetch_all(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        overlap.len(),
        1,
        "the old in-place shift would target an occupied member position before prefix insertion"
    );
    assert_eq!(overlap[0].3 - overlap[0].2, prefix.captured_seq);
    let own_positions:Vec<(Id,i64,Option<String>,Json)>=sqlx::query_as("SELECT id,seq_created,native_ref,payload FROM items WHERE thread_id=$1 AND turn_id IS NOT NULL ORDER BY seq_created")
        .bind(side).fetch_all(rig.world.pool.pool()).await.unwrap();
    assert!(
        own_positions.len() > parent_positions.len(),
        "this side has more actual history than its retained parent prefix"
    );
    rig.say(parent, "Excluded later parent suffix").await;
    rig.world
        .run_turn(parent, 1, "Excluded parent answer")
        .await;
    accounted(&rig, parent).await;
    let plan = rig.plan(side, &rig.fork()).await;
    assert_eq!(
        plan.history.turns, 3,
        "the captured parent prefix and two actual side turns form the full history"
    );
    let began = rig.world.began(CommandKind::SessionOpenFenced);
    rig.world
        .machine
        .handler
        .hold(CommandKind::SessionOpenFenced);
    let job = rig.confirm(side, &plan).await;
    rig.world
        .wait_began(CommandKind::SessionOpenFenced, began + 1)
        .await;
    // The prefix is held through the prefix cut's spans: its items keep their own positions, at or below the
    // prefix cut's capture, and the side thread's own items follow them.
    let retained: Vec<(Id, i64)> = sqlx::query_as(
        "SELECT item_id,seq_created FROM thread_history_cut_members($1) ORDER BY seq_created",
    )
    .bind(job.id)
    .fetch_all(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        retained
            .iter()
            .filter(|(_, position)| *position <= prefix.captured_seq)
            .copied()
            .collect::<Vec<_>>(),
        parent_positions,
        "the original parent item IDs and positions are retained exactly"
    );
    let frozen = cuts::items_between(
        &mut rig.world.pool.acquire().await.unwrap(),
        job.id,
        0,
        i64::MAX,
        None,
    )
    .await
    .unwrap();
    for (id, position, native, payload) in &own_positions {
        assert!(
            retained.contains(&(*id, *position + prefix.captured_seq)),
            "every own-history item is admitted directly at its final position"
        );
        let item = frozen.iter().find(|item| item.id == *id).unwrap();
        assert_eq!(item.seq_created, *position + prefix.captured_seq);
        assert_eq!(&item.native_ref, native);
        assert!(
            &item.payload == payload,
            "capturing a final position preserves the source's exact native identity and frozen body"
        );
    }
    rig.say(side, "Excluded later side input").await;
    rig.world
        .run_turn(side, 2, "Excluded later side answer")
        .await;
    rig.world.wait_state(side, "idle").await;
    accounted(&rig, side).await;
    let pending = rig
        .world
        .engine()
        .submit_input(&rig.caller(), side, queued("New actual side work"))
        .await
        .unwrap();
    rig.world.machine.handler.release();
    rig.wait_job(job.id, "succeeded").await;
    let target = rig.new_thread_of(&job);
    let visible = rig.world.sync(target).await;
    let words = common::said(&visible).join("\n");
    for included in [
        "Plan the work",
        "The plan is ready",
        "Side input",
        "Side answer",
    ] {
        assert!(words.contains(included));
    }
    for excluded in [
        "Excluded later parent suffix",
        "Excluded parent answer",
        "Excluded later side input",
        "Excluded later side answer",
        "New actual side work",
    ] {
        assert!(!words.contains(excluded));
    }
    let mut expected = vec![
        "user: Plan the work".to_owned(),
        "agent: The plan is ready".to_owned(),
        "user: Side input".to_owned(),
        "agent: Side answer".to_owned(),
        "user: Longer side input".to_owned(),
    ];
    expected.extend(answers.iter().map(|answer| format!("agent: {answer}")));
    assert_eq!(
        common::said(&visible),
        expected,
        "the entire copied conversation retains exact prefix/side order without omitted or duplicate messages"
    );
    assert_eq!(visible.turns.len(), 3);
    assert!(
        !rig.world
            .scalar::<bool>("SELECT deleted_at IS NOT NULL FROM nodes WHERE id=$1", side)
            .await
    );
    assert_eq!(
        rig.job(job.id).await.plan["carried_history_range"],
        json!([1, 3]),
        "the reported cut includes its captured parent prefix and both side turns"
    );
    let source = rig.world.sync(side).await;
    let source_words = common::said(&source);
    for answer in &answers {
        assert!(source_words.contains(&format!("agent: {answer}")));
    }
    assert!(
        source_words.contains(&"user: Excluded later side input".to_owned())
            && source_words.contains(&"agent: Excluded later side answer".to_owned()),
        "the source's later completed work remains in its own live history"
    );
    let source_ids = own_positions
        .iter()
        .map(|(id, _, _, _)| *id)
        .collect::<Vec<_>>();
    let original_rows:Vec<(Id,i64,Option<String>,Json)>=sqlx::query_as("SELECT id,seq_created,native_ref,payload FROM items WHERE thread_id=$1 AND id=ANY($2) ORDER BY seq_created")
        .bind(side).bind(source_ids).fetch_all(rig.world.pool.pool()).await.unwrap();
    assert!(
        original_rows == own_positions,
        "fork capture/copy does not renumber or rewrite the original source records"
    );
    let owner: Id = rig
        .world
        .scalar(
            "SELECT thread_id FROM thread_inputs WHERE id=$1",
            pending.input.id,
        )
        .await;
    assert_eq!(owner, side);
    let copies: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM thread_inputs WHERE thread_id=$1 AND content @> $2",
    )
    .bind(target)
    .bind(json!([{"type":"text","text":"New actual side work"}]))
    .fetch_one(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(copies, 0);
    let ids: Vec<(Id, Id)> =
        sqlx::query_as("SELECT t.id,t.continuation_id FROM turns t WHERE t.thread_id=$1")
            .bind(target)
            .fetch_all(rig.world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        ids.iter()
            .map(|(_, owner)| *owner)
            .collect::<BTreeSet<_>>()
            .len(),
        1
    );
    rig.world.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_a_side_keeps_its_uncertain_native_inputs_until_the_original_queue_actually_settles()
 {
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Default::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("Closed side native inputs").await;
    accounted(&rig, parent).await;
    rig.world
        .machine
        .handler
        .hold(CommandKind::SessionOpenFenced);
    let started = rig
        .world
        .engine()
        .start_side_thread(
            &rig.caller(),
            parent,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: "Keep these original side words".into(),
            }],
            "closed-side-native-inputs".into(),
            None,
        )
        .await
        .unwrap();
    rig.world
        .wait_began(CommandKind::SessionOpenFenced, 1)
        .await;
    let cut = cuts::cut(&mut rig.world.pool.acquire().await.unwrap(), started.thread)
        .await
        .unwrap()
        .unwrap();
    let command: Outgoing =
        serde_json::from_value(cut.artifacts["side_open_neutral"].clone()).unwrap();
    let opened: orch_link_contract::continuations::SessionOpenFenced =
        serde_json::from_value(command.body.clone()).unwrap();
    assert_eq!(opened.thread_id, started.thread);
    let archive = opened.session.history.unwrap();
    let bytes = rig.blob_text(&archive.blob).await;
    let pins = storage::blob(&mut rig.world.pool.acquire().await.unwrap(), &archive.blob)
        .await
        .unwrap()
        .unwrap()
        .pin_count;
    assert!(pins > 0);
    assert!(
        rig.world
            .engine()
            .close_side_thread(started.thread, "closed", Some(rig.world.ada), None)
            .await
            .unwrap()
    );
    {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        let retained: (bool, Json) = sqlx::query_as(
            "SELECT NOT h.settled,to_jsonb(c) FROM thread_history_cut_commands h
            JOIN commands c ON c.id=h.command_id WHERE h.cut_id=$1 AND h.command_id=$2",
        )
        .bind(started.thread)
        .bind(command.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(
            retained.0,
            "controller close/cancellation alone is not native completion"
        );
        assert_eq!(retained.1["body"], command.body);
        let original = orch_db::queries::link::command(&mut conn, command.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            Some(orch_types::Time::new(original.deadline_at)),
            command.deadline
        );
        let idle = rig.world.clock.now() + chrono::Duration::seconds(1);
        assert!(
            !storage::forget_blob(&mut conn, &archive.blob, idle)
                .await
                .unwrap()
        );
        orch_db::queries::link::purge_finished(&mut conn, idle, 1000)
            .await
            .unwrap();
        assert!(
            orch_db::queries::link::command(&mut conn, command.id)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            storage::blob(&mut conn, &archive.blob)
                .await
                .unwrap()
                .unwrap()
                .pin_count,
            pins
        );
    }
    assert_eq!(rig.blob_text(&archive.blob).await, bytes);
    rig.world.machine.handler.release();
    eventually(
        "the actual closed-side native outcome or same-key queue barrier settles its hold",
        || async {
            sqlx::query_scalar::<_, bool>(
                "SELECT settled FROM thread_history_cut_commands WHERE cut_id=$1 AND command_id=$2",
            )
            .bind(started.thread)
            .bind(command.id)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap_or(false)
        },
    )
    .await;
    let (row, fence) = {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        let row = orch_db::queries::link::command(&mut conn, command.id)
            .await
            .unwrap()
            .unwrap();
        let fence: Option<Id> = sqlx::query_scalar(
            "SELECT fence_id FROM thread_history_cut_commands WHERE command_id=$1",
        )
        .bind(command.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        (row, fence)
    };
    assert!(row.body == command.body);
    assert_eq!(
        Some(orch_types::Time::new(row.deadline_at)),
        command.deadline
    );
    let actual = rig
        .world
        .commands
        .as_engine()
        .status(command.id)
        .await
        .unwrap()
        .and_then(|info| info.outcome);
    let actual_completion = matches!(&actual, Some(Outcome::Succeeded(_)))
        || matches!(&actual,Some(Outcome::Failed(error)) if error.code=="unsupported");
    if !actual_completion {
        let fence = fence.expect("a cancelled/unknown original needs an actual completion barrier");
        let barrier =
            orch_db::queries::link::command(&mut rig.world.pool.acquire().await.unwrap(), fence)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            (barrier.machine_id, barrier.serial_key()),
            (row.machine_id, row.serial_key())
        );
        assert_eq!(barrier.kind, CommandKind::MachineProbe.as_str());
        assert_eq!(barrier.body, json!({}));
        assert!(matches!(
            rig.world
                .commands
                .as_engine()
                .status(fence)
                .await
                .unwrap()
                .and_then(|info| info.outcome),
            Some(Outcome::Succeeded(_))
        ));
    }
    assert!(
        rig.world.turn_starts(started.thread).is_empty(),
        "closing cannot deliver the original side words or start an empty replacement"
    );
    assert!(
        rig.opens()
            .iter()
            .filter(|open| open.thread_id == started.thread && open.mode != SessionMode::Resume)
            .count()
            <= 1
    );
    assert_eq!(
        storage::blob(&mut rig.world.pool.acquire().await.unwrap(), &archive.blob)
            .await
            .unwrap()
            .unwrap()
            .pin_count,
        pins,
        "the deleted side still retains its readable original cut; settlement cannot double-release its owner"
    );
    rig.world.shutdown().await;
}

#[tokio::test]
async fn carry_uses_the_proved_fallback_even_when_an_old_profile_claims_native_import() {
    let can = Can {
        export_native: true,
        import_native: true,
        import_neutral: true,
        ..Default::default()
    };
    let rig = Rig::new(can, "codex", can).await;
    let thread = rig.thread_with_history("Legacy native import claim").await;
    accounted(&rig, thread).await;
    // This synthetic native peer retains an old declaration, without any kept build's proof.
    sqlx::query("UPDATE agents SET harness_kind='codex',descriptor=$2 WHERE id=$1")
        .bind(rig.world.agent)
        .bind(descriptor_with("codex", can))
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE providers SET harness_kind='codex' WHERE id=$1")
        .bind(rig.world.provider)
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE agents SET descriptor_overrides=$2 WHERE id=$1")
        .bind(rig.other_agent)
        .bind(json!({"capabilities":{"import_native":true}}))
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.history.method, HistoryMethod::NeutralImport);
    let mut target = descriptor_with("codex", can);
    target["capabilities"]["import_neutral"] = json!(false);
    sqlx::query("UPDATE agents SET descriptor=$2 WHERE id=$1")
        .bind(rig.other_agent)
        .bind(target)
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.history.method, HistoryMethod::FileHandoff);
    rig.world.shutdown().await;
}
