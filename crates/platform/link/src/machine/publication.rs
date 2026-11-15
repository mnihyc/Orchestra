//! Producers retain ownership until whole records have durable receipts; controls keep running.
use super::*;
use crate::spool::{SpoolError, SpoolEvent, SyncBatch};
use orch_api_contract::resources::{SpoolProblem, SpoolStatus};
use orch_types::{Hash, Time};
use std::collections::{BTreeSet, VecDeque};

struct Pending {
    stream: Stream,
    event: SpoolEvent,
    reply: oneshot::Sender<Result<(), PublishError>>,
}
struct Wave {
    task: tokio::task::JoinHandle<std::io::Result<SyncBatch>>,
    count: usize,
    frames: Vec<Frame>,
    session: Option<SessionSender>,
    epoch: Id,
}
pub(super) struct Completion {
    wave: Wave,
    result: std::io::Result<SyncBatch>,
}

pub(super) struct Publications {
    pending: VecDeque<Pending>,
    waiting: VecDeque<oneshot::Sender<Result<(), PublishError>>>,
    frames: Vec<Frame>,
    wave: Option<Wave>,
    problem: Option<SpoolProblem>,
    append_blocked: bool,
    pump_blocked: bool,
    rejected: BTreeSet<(String, String, Hash)>,
    append_retry_at: Instant,
    force_retry_at: Instant,
    epoch: Id,
}

impl Publications {
    pub fn new(problem: Option<SpoolProblem>) -> Self {
        Self {
            pending: VecDeque::new(),
            waiting: VecDeque::new(),
            frames: Vec::new(),
            wave: None,
            problem,
            append_blocked: false,
            pump_blocked: false,
            rejected: BTreeSet::new(),
            append_retry_at: Instant::now(),
            force_retry_at: Instant::now(),
            epoch: orch_types::new_id(),
        }
    }

    pub fn push(
        &mut self,
        stream: Stream,
        event: &Event,
        at: Time,
        reply: oneshot::Sender<Result<(), PublishError>>,
    ) {
        match event.body() {
            Ok(body) => self.pending.push_back(Pending {
                stream,
                event: SpoolEvent {
                    at,
                    kind: event.kind().as_str().into(),
                    body,
                },
                reply,
            }),
            Err(_) => {
                self.rejected.insert((
                    stream.to_string(),
                    event.kind().as_str().into(),
                    Hash::of(b"unencodable native event"),
                ));
                self.problem = Some(SpoolProblem::EventRejected);
                let _ = reply.send(Err(PublishError::Spool(SpoolError::Io {
                    path: "event".into(),
                    source: std::io::Error::other("the native event could not be encoded"),
                })));
            }
        }
    }

    pub fn session_changed(&mut self) {
        self.epoch = orch_types::new_id();
        self.frames.clear();
        // The new session replays retained records with its own limits and cursors.
    }

    pub fn drive(&mut self, streams: &mut StreamSender, now: Instant) {
        if now < self.append_retry_at {
            return;
        }
        self.append_blocked = false;
        while let Some(next) = self.pending.front() {
            match streams.publish(
                &next.stream,
                next.event.at,
                &next.event.kind,
                next.event.body.clone(),
                now,
            ) {
                Ok(frames) => {
                    if self.rejected.iter().any(|(stream, kind, _)| {
                        stream == &next.stream.to_string() && kind == &next.event.kind
                    }) {
                        self.rejected.remove(&rejected_key(next));
                    }
                    self.frames.extend(frames);
                    if let Some(done) = self.pending.pop_front() {
                        self.waiting.push_back(done.reply);
                    }
                }
                Err(PublishError::Spool(SpoolError::Io { source, .. })) => {
                    self.problem = Some(io_problem(&source, SpoolProblem::Write));
                    self.append_blocked = true;
                    self.append_retry_at = now + CORE_TICK;
                    break;
                }
                Err(error) => {
                    // Admission errors remain explicit. The harness retains and retries its batch.
                    let fresh = self.rejected.insert(rejected_key(next));
                    if fresh {
                        if let PublishError::TooLarge { bytes, max } = &error {
                            tracing::error!(
                                kind = next.event.kind,
                                bytes,
                                max,
                                "an event was not published: it is larger than a message may be"
                            );
                            metrics::counter!("orch_link_events_refused_total","kind"=>next.event.kind.clone()).increment(1);
                        } else {
                            tracing::error!(
                                kind = next.event.kind,
                                "an event could not be admitted and remains with its producer"
                            );
                        }
                    }
                    self.problem = Some(SpoolProblem::EventRejected);
                    if let Some(done) = self.pending.pop_front() {
                        let _ = done.reply.send(Err(error));
                    }
                }
            }
        }
    }

    pub fn force(
        &mut self,
        streams: &mut StreamSender,
        session: Option<&SessionSender>,
        now: Instant,
    ) {
        if self.wave.is_some() || now < self.force_retry_at {
            return;
        }
        self.pump_blocked = false;
        if let Some(session) = session.filter(|_| now >= self.append_retry_at) {
            if let Err(error) =
                streams.pump_into(session.event_frame_bytes(), now, &mut self.frames)
            {
                self.problem = Some(spool_problem(&error, SpoolProblem::Write));
                self.pump_blocked = true;
                self.append_retry_at = now + CORE_TICK;
            }
        }
        if !streams.is_dirty() && self.waiting.is_empty() && self.frames.is_empty() {
            return;
        }
        match streams.sync_handles() {
            Ok(batch) => {
                self.wave = Some(Wave {
                    task: tokio::task::spawn_blocking(move || {
                        batch.force()?;
                        Ok(batch)
                    }),
                    count: self.waiting.len(),
                    frames: std::mem::take(&mut self.frames),
                    session: session.cloned(),
                    epoch: self.epoch,
                });
            }
            Err(error) => {
                self.problem = Some(spool_problem(&error, SpoolProblem::Write));
                self.force_retry_at = now + CORE_TICK;
            }
        }
    }

    /// Cancellation by select leaves the task, frames and receipts owned here.
    pub async fn completed(&mut self) -> Completion {
        let result = match self.wave.as_mut() {
            Some(wave) => (&mut wave.task)
                .await
                .map_err(std::io::Error::other)
                .and_then(|result| result),
            None => return std::future::pending().await,
        };
        match self.wave.take() {
            Some(wave) => Completion { wave, result },
            None => std::future::pending().await,
        }
    }

    pub fn finish(&mut self, completion: Completion, streams: &mut StreamSender) {
        let Completion { wave, result } = completion;
        match result {
            Ok(batch) => {
                streams.complete_sync(&batch);
                for _ in 0..wave.count {
                    if let Some(reply) = self.waiting.pop_front() {
                        let _ = reply.send(Ok(()));
                    }
                }
                if wave.epoch == self.epoch {
                    send_all(wave.session.as_ref(), wave.frames);
                }
                if !self.append_blocked && !self.pump_blocked {
                    self.problem =
                        (!self.rejected.is_empty()).then_some(SpoolProblem::EventRejected);
                }
            }
            Err(error) => {
                self.problem = Some(io_problem(&error, SpoolProblem::Sync));
                self.force_retry_at = Instant::now() + CORE_TICK;
                if wave.epoch == self.epoch {
                    let _ = self.frames.splice(0..0, wave.frames);
                }
                // Every reply and every dirty revision remains pending for the next forcing attempt.
            }
        }
    }

    pub fn status(&self) -> SpoolStatus {
        SpoolStatus {
            problem: self.problem,
            waiting_events: u64::try_from(self.pending.len().saturating_add(self.waiting.len()))
                .unwrap_or(u64::MAX),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
            && self.waiting.is_empty()
            && self.wave.is_none()
            && self.frames.is_empty()
    }
}

fn io_problem(error: &std::io::Error, otherwise: SpoolProblem) -> SpoolProblem {
    if matches!(
        error.kind(),
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded
    ) {
        SpoolProblem::Full
    } else if error.kind() == std::io::ErrorKind::InvalidData {
        SpoolProblem::Damaged
    } else {
        otherwise
    }
}
fn spool_problem(error: &SpoolError, otherwise: SpoolProblem) -> SpoolProblem {
    match error {
        SpoolError::Io { source, .. } => io_problem(source, otherwise),
        _ => otherwise,
    }
}
fn rejected_key(pending: &Pending) -> (String, String, Hash) {
    (
        pending.stream.to_string(),
        pending.event.kind.clone(),
        Hash::of(pending.event.body.to_string().as_bytes()),
    )
}
