//! Default-off, process-local test observations. No payload, credential or product state is retained.
//!
//! A dedicated test installs one scoped recorder before constructing the real TLS fixture.
//! Queue counts and encoded payload bytes observe the existing queues; they impose no limit
//! and do not make an unbounded queue bounded. Unknown sizes and discarded work stay explicit.

use crate::framing::{Connection, Message, MessageSink, MessageSource, TransportError};
use async_trait::async_trait;
use orch_link_contract::backplane::{BackplaneFrame, DownFrame, UpFrame};
use orch_link_contract::frames::Frame;
use orch_types::Id;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::Instant;

pub mod process;

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

static ACTIVE: OnceLock<Mutex<Option<Weak<Recorder>>>> = OnceLock::new();
#[cfg(test)]
static RECORDING_TEST_LOCK: Mutex<()> = Mutex::new(());

fn active() -> Option<Arc<Recorder>> {
    lock(ACTIVE.get_or_init(|| Mutex::new(None)))
        .as_ref()
        .and_then(Weak::upgrade)
}

/// Immutable counters captured only when receiver teardown observed a complete empty queue.
/// Later completion cannot retroactively qualify an in-flight or discarded close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DrainedRetirement {
    pub enqueue_attempts: usize,
    pub selected: usize,
    pub sent: usize,
    pub processed: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct QueueSnapshot {
    pub label: &'static str,
    pub queued_messages: usize,
    pub queued_payload_bytes: usize,
    pub queued_unknown_sizes: usize,
    pub peak_messages: usize,
    pub peak_payload_bytes: usize,
    pub unknown_sizes_seen: usize,
    pub in_flight_messages: usize,
    pub in_flight_payload_bytes: usize,
    pub in_flight_unknown_sizes: usize,
    /// Queued plus selected logical payload; selected batches still retain their bytes.
    pub peak_retained_payload_bytes: usize,
    pub enqueue_attempts: usize,
    pub selected: usize,
    pub sent: usize,
    /// A local consumer finished handling or handed off the selected item; not a wire send.
    pub processed: usize,
    pub failed_enqueues: usize,
    pub selected_without_completion: usize,
    pub discarded_on_receiver_close: usize,
    pub receiver_closed: bool,
    pub drained_retirement: Option<DrainedRetirement>,
    pub inconsistent: bool,
    /// A concurrent send crossed receiver teardown; close/discard conservation is unknown.
    pub indeterminate_close_race: bool,
}

impl QueueSnapshot {
    pub(crate) fn conserved_empty(&self) -> bool {
        self.queued_messages == 0
            && self.queued_payload_bytes == 0
            && self.queued_unknown_sizes == 0
            && self.in_flight_messages == 0
            && self.in_flight_payload_bytes == 0
            && self.in_flight_unknown_sizes == 0
            && self.unknown_sizes_seen == 0
            && !self.inconsistent
            && !self.indeterminate_close_race
            && self.discarded_on_receiver_close == 0
            && self.selected_without_completion == 0
            && self.failed_enqueues == 0
            && self.selected == self.sent + self.processed
            && self.enqueue_attempts == self.selected
    }

    fn retirement_counters(&self) -> DrainedRetirement {
        DrainedRetirement {
            enqueue_attempts: self.enqueue_attempts,
            selected: self.selected,
            sent: self.sent,
            processed: self.processed,
        }
    }

    pub(crate) fn drain_or_retired(&self) -> bool {
        self.conserved_empty()
            && if self.receiver_closed {
                self.drained_retirement.as_ref() == Some(&self.retirement_counters())
            } else {
                self.drained_retirement.is_none()
            }
    }
}

#[derive(Debug)]
pub struct QueueGauge(Mutex<QueueSnapshot>);

impl QueueGauge {
    pub fn new(label: &'static str) -> Arc<Self> {
        let gauge = Arc::new(Self(Mutex::new(QueueSnapshot {
            label,
            ..QueueSnapshot::default()
        })));
        if let Some(recorder) = active() {
            let mut state = lock(&recorder.state);
            if state.queues.len() < recorder.max_queues {
                state.queues.push(Arc::clone(&gauge));
            } else {
                state.overflowed = true;
            }
        }
        gauge
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        lock(&self.0).clone()
    }

    pub fn enqueue(&self, bytes: Option<usize>) {
        let mut state = lock(&self.0);
        state.enqueue_attempts += 1;
        if state.receiver_closed {
            state.indeterminate_close_race = true;
            return;
        }
        state.queued_messages += 1;
        match bytes {
            Some(bytes) => state.queued_payload_bytes += bytes,
            None => {
                state.queued_unknown_sizes += 1;
                state.unknown_sizes_seen += 1;
            }
        }
        state.peak_messages = state.peak_messages.max(state.queued_messages);
        state.peak_payload_bytes = state.peak_payload_bytes.max(state.queued_payload_bytes);
        state.peak_retained_payload_bytes = state
            .peak_retained_payload_bytes
            .max(state.queued_payload_bytes + state.in_flight_payload_bytes);
    }

    fn remove(state: &mut QueueSnapshot, bytes: Option<usize>) {
        if state.receiver_closed {
            return;
        }
        state.inconsistent |= state.queued_messages == 0;
        state.queued_messages = state.queued_messages.saturating_sub(1);
        match bytes {
            Some(bytes) => {
                state.inconsistent |= state.queued_payload_bytes < bytes;
                state.queued_payload_bytes = state.queued_payload_bytes.saturating_sub(bytes);
            }
            None => {
                state.inconsistent |= state.queued_unknown_sizes == 0;
                state.queued_unknown_sizes = state.queued_unknown_sizes.saturating_sub(1);
            }
        }
    }

    pub fn failed_enqueue(&self, bytes: Option<usize>) {
        let mut state = lock(&self.0);
        state.indeterminate_close_race |= state.receiver_closed;
        Self::remove(&mut state, bytes);
        state.failed_enqueues += 1;
    }

    pub fn selected(self: &Arc<Self>, bytes: Option<usize>) -> Flight {
        let mut state = lock(&self.0);
        Self::remove(&mut state, bytes);
        state.selected += 1;
        state.in_flight_messages += 1;
        match bytes {
            Some(bytes) => state.in_flight_payload_bytes += bytes,
            None => state.in_flight_unknown_sizes += 1,
        }
        Flight {
            gauge: Arc::clone(self),
            complete: false,
            processed: false,
            bytes,
        }
    }

    pub fn receiver(self: &Arc<Self>) -> ReceiverLifetime {
        ReceiverLifetime(Arc::clone(self))
    }

    pub fn send<T>(
        &self,
        sender: &tokio::sync::mpsc::UnboundedSender<T>,
        item: T,
        bytes: Option<usize>,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<T>> {
        self.enqueue(bytes);
        let result = sender.send(item);
        if result.is_err() {
            self.failed_enqueue(bytes);
        }
        result
    }
}

/// Selection is separate from completion. An incomplete selection can mean cancellation,
/// a sink failure or encoding rejection; the counter does not invent which occurred.
pub struct Flight {
    gauge: Arc<QueueGauge>,
    complete: bool,
    processed: bool,
    bytes: Option<usize>,
}
impl Flight {
    pub fn complete(mut self) {
        self.complete = true;
    }
    pub fn processed(mut self) {
        self.processed = true;
    }
}
impl Drop for Flight {
    fn drop(&mut self) {
        let mut state = lock(&self.gauge.0);
        state.inconsistent |= state.in_flight_messages == 0;
        state.in_flight_messages = state.in_flight_messages.saturating_sub(1);
        match self.bytes {
            Some(bytes) => {
                state.inconsistent |= state.in_flight_payload_bytes < bytes;
                state.in_flight_payload_bytes = state.in_flight_payload_bytes.saturating_sub(bytes);
            }
            None => {
                state.inconsistent |= state.in_flight_unknown_sizes == 0;
                state.in_flight_unknown_sizes = state.in_flight_unknown_sizes.saturating_sub(1);
            }
        }
        if self.processed {
            state.processed += 1;
        } else if self.complete {
            state.sent += 1;
        } else {
            state.selected_without_completion += 1;
        }
    }
}

#[derive(Debug)]
pub struct ReceiverLifetime(Arc<QueueGauge>);
impl ReceiverLifetime {
    pub fn selected(&self, bytes: Option<usize>) -> Flight {
        self.0.selected(bytes)
    }
}
impl Drop for ReceiverLifetime {
    fn drop(&mut self) {
        let mut state = lock(&self.0.0);
        // Record before clearing queued payload or allowing later Flight completion.
        if !state.receiver_closed && state.conserved_empty() {
            state.drained_retirement = Some(state.retirement_counters());
        }
        let discarded = state.queued_messages;
        state.discarded_on_receiver_close += discarded;
        state.queued_messages = 0;
        state.queued_payload_bytes = 0;
        state.queued_unknown_sizes = 0;
        state.receiver_closed = true;
    }
}

#[derive(Clone, Copy)]
enum Phase {
    ApiSend = 0,
    GatewaySend = 1,
    GatewayAck = 2,
    ApiAck = 3,
}
#[derive(Default)]
struct CommandTimes {
    at: [Option<Instant>; 4],
    seen: [usize; 4],
    request: Option<Id>,
    machine: Option<Id>,
    session: Option<Id>,
}
#[derive(Default)]
struct State {
    commands: HashMap<Id, CommandTimes>,
    queues: Vec<Arc<QueueGauge>>,
    overflowed: bool,
    actual_connector_machine: Option<Id>,
}
pub struct Recorder {
    state: Mutex<State>,
    max_queues: usize,
    owned_probes: Option<(Id, usize)>,
}
pub struct Recording {
    pub recorder: Arc<Recorder>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandSnapshot {
    pub id: Id,
    pub seen: [usize; 4],
    pub carrier_ack_micros: Option<u128>,
    pub api_ack_micros: Option<u128>,
    pub request_id: Option<Id>,
    pub machine_id: Option<Id>,
    pub gateway_session_id: Option<Id>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Snapshot {
    pub commands: Vec<CommandSnapshot>,
    pub queues: Vec<QueueSnapshot>,
    pub overflowed: bool,
    pub actual_connector_machine: Option<Id>,
}

impl Recording {
    /// IDs and queue-count ceiling bound the recorder itself, not the product queues.
    pub fn install(ids: &[Id], max_queues: usize) -> Result<Self, &'static str> {
        if ids.is_empty() || max_queues == 0 {
            return Err("the dedicated fixture must bound its observations");
        }
        Self::install_inner(ids, max_queues, None)
    }

    pub(crate) fn for_machine(
        machine: Id,
        max_commands: usize,
        max_queues: usize,
    ) -> Result<Self, &'static str> {
        if machine.is_nil() || max_commands == 0 || max_queues == 0 {
            return Err("the owned QA invocation must bound its machine and observations");
        }
        Self::install_inner(&[], max_queues, Some((machine, max_commands)))
    }

    fn install_inner(
        ids: &[Id],
        max_queues: usize,
        owned_probes: Option<(Id, usize)>,
    ) -> Result<Self, &'static str> {
        let slot = ACTIVE.get_or_init(|| Mutex::new(None));
        let mut slot = lock(slot);
        if slot.as_ref().and_then(Weak::upgrade).is_some() {
            return Err("another observed fixture is active");
        }
        let recorder = Arc::new(Recorder {
            state: Mutex::new(State {
                commands: ids
                    .iter()
                    .copied()
                    .map(|id| (id, CommandTimes::default()))
                    .collect(),
                ..State::default()
            }),
            max_queues,
            owned_probes,
        });
        *slot = Some(Arc::downgrade(&recorder));
        Ok(Self { recorder })
    }
}
impl Drop for Recording {
    fn drop(&mut self) {
        let mut slot = lock(ACTIVE.get_or_init(|| Mutex::new(None)));
        if slot
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|value| Arc::ptr_eq(&value, &self.recorder))
        {
            *slot = None;
        }
    }
}

impl Recorder {
    fn register_probe(
        &self,
        machine: Id,
        command: Id,
        kind: &str,
        request: Option<Id>,
        session: Option<Id>,
    ) {
        let Some((owned, limit)) = self.owned_probes else {
            return;
        };
        if owned != machine || kind != "machine.probe" {
            return;
        }
        let mut state = lock(&self.state);
        if !state.commands.contains_key(&command) && state.commands.len() >= limit {
            state.overflowed = true;
            return;
        }
        let times = state.commands.entry(command).or_default();
        if times.machine.is_some_and(|previous| previous != machine)
            || (times.request.is_some() && request.is_some() && times.request != request)
            || (times.session.is_some() && session.is_some() && times.session != session)
        {
            state.overflowed = true; // Conflicting correlation cannot become complete evidence.
            return;
        }
        times.machine = Some(machine);
        times.request = times.request.or(request);
        times.session = times.session.or(session);
    }
    fn mark(&self, id: Id, phase: Phase) {
        if let Some(command) = lock(&self.state).commands.get_mut(&id) {
            let index = phase as usize;
            command.seen[index] += 1;
            command.at[index].get_or_insert_with(Instant::now);
        }
    }
    fn mark_machine(&self, id: Id, phase: Phase, machine: Id) {
        if self.owned_probes.is_none_or(|(owned, _)| owned == machine) {
            self.mark(id, phase);
        }
    }
    fn api_ack(&self, id: Id, machine: Id, session: Id) {
        if self.owned_probes.is_some_and(|(owned, _)| owned != machine) {
            return;
        }
        if self.owned_probes.is_some() {
            let mut state = lock(&self.state);
            if let Some(command) = state.commands.get_mut(&id) {
                if command.session.is_some_and(|previous| previous != session) {
                    state.overflowed = true;
                    return;
                }
                command.session = Some(session);
            }
        }
        self.mark(id, Phase::ApiAck);
    }
    pub fn snapshot(&self) -> Snapshot {
        let state = lock(&self.state);
        let elapsed = |from: Option<Instant>, to: Option<Instant>| {
            from.zip(to)
                .and_then(|(from, to)| to.checked_duration_since(from))
                .map(|value| value.as_micros())
        };
        Snapshot {
            commands: state
                .commands
                .iter()
                .map(|(id, value)| CommandSnapshot {
                    id: *id,
                    seen: value.seen,
                    carrier_ack_micros: elapsed(value.at[1], value.at[2]),
                    api_ack_micros: elapsed(value.at[0], value.at[3]),
                    request_id: value.request,
                    machine_id: value.machine,
                    gateway_session_id: value.session,
                })
                .collect(),
            queues: state.queues.iter().map(|queue| queue.snapshot()).collect(),
            overflowed: state.overflowed,
            actual_connector_machine: state.actual_connector_machine,
        }
    }
}

tokio::task_local! {
    static PROBE_REQUEST: (Id, Id);
}

/// Scope only the already-authorized public MachineProbe future; no body or token is retained.
pub async fn probe_request<F: std::future::Future>(
    machine: Id,
    request: &str,
    work: F,
) -> F::Output {
    match request.parse::<Id>() {
        Ok(request) => PROBE_REQUEST.scope((machine, request), work).await,
        Err(_) => work.await,
    }
}

/// Called after the real command row commits and before its original adoption.
pub fn issued_probe(machine: Id, command: Id, kind: &str) {
    if let (Some(recorder), Ok((selected, request))) =
        (active(), PROBE_REQUEST.try_with(|scope| *scope))
        && selected == machine
    {
        recorder.register_probe(machine, command, kind, Some(request), None);
    }
}

/// The connector's actual enrolled identity, supplied by its original MachineConfig.
pub fn machine_started(machine: Id) {
    if let Some(recorder) = active() {
        let mut state = lock(&recorder.state);
        let mismatch = recorder
            .owned_probes
            .is_some_and(|(owned, _)| owned != machine)
            || state
                .actual_connector_machine
                .is_some_and(|previous| previous != machine);
        state.overflowed |= mismatch;
        state.actual_connector_machine = Some(machine);
    }
}

pub fn api_sending(frame: &BackplaneFrame) {
    if let BackplaneFrame::Down(down) = frame
        && let DownFrame::Cmd(cmd) = &down.frame
        && let Some(recorder) = active()
    {
        recorder.mark_machine(cmd.id, Phase::ApiSend, down.machine_id);
    }
}
pub fn api_received(frame: &BackplaneFrame) {
    if let BackplaneFrame::Up(up) = frame
        && let UpFrame::CmdAck(ack) = &up.frame
        && let Some(recorder) = active()
    {
        recorder.api_ack(ack.id, up.machine_id, up.session_id);
    }
}

struct ObservedSink {
    inner: Box<dyn MessageSink>,
    recorder: Arc<Recorder>,
    identity: Arc<Mutex<Option<(Id, Id)>>>,
}
struct ObservedSource {
    inner: Box<dyn MessageSource>,
    recorder: Arc<Recorder>,
    identity: Arc<Mutex<Option<(Id, Id)>>>,
}
#[async_trait]
impl MessageSink for ObservedSink {
    async fn ready(&mut self) {
        self.inner.ready().await;
    }
    async fn send(&mut self, message: Message) -> Result<(), TransportError> {
        if let Message::Text(text) = &message
            && let Ok(Frame::Cmd(cmd)) = serde_json::from_str(text)
        {
            if let Some((machine, session)) = *lock(&self.identity) {
                self.recorder
                    .register_probe(machine, cmd.id, &cmd.kind, None, Some(session));
                self.recorder
                    .mark_machine(cmd.id, Phase::GatewaySend, machine);
            } else if self.recorder.owned_probes.is_none() {
                self.recorder.mark(cmd.id, Phase::GatewaySend);
            }
        }
        self.inner.send(message).await
    }
    async fn close(&mut self) {
        self.inner.close().await;
    }
}
#[async_trait]
impl MessageSource for ObservedSource {
    async fn recv(&mut self) -> Result<Option<Message>, TransportError> {
        let message = self.inner.recv().await?;
        if let Some(Message::Text(text)) = &message
            && let Ok(Frame::CmdAck(ack)) = serde_json::from_str(text)
        {
            if let Some((machine, _)) = *lock(&self.identity) {
                self.recorder
                    .mark_machine(ack.id, Phase::GatewayAck, machine);
            } else if self.recorder.owned_probes.is_none() {
                self.recorder.mark(ack.id, Phase::GatewayAck);
            }
        }
        Ok(message)
    }
}

pub struct GatewayObservation(Arc<Mutex<Option<(Id, Id)>>>);
impl GatewayObservation {
    /// Only the gateway's successful existing session authentication supplies this identity.
    pub fn accepted(&self, machine: Id, session: Id) {
        *lock(&self.0) = Some((machine, session));
    }
}

pub fn gateway_connection(connection: Connection) -> (Connection, GatewayObservation) {
    let identity = Arc::new(Mutex::new(None));
    let observation = GatewayObservation(Arc::clone(&identity));
    let Some(recorder) = active() else {
        return (connection, observation);
    };
    (
        Connection {
            sink: Box::new(ObservedSink {
                inner: connection.sink,
                recorder: Arc::clone(&recorder),
                identity: Arc::clone(&identity),
            }),
            source: Box::new(ObservedSource {
                inner: connection.source,
                recorder,
                identity,
            }),
            peer: connection.peer,
        },
        observation,
    )
}

/// Logical serialized payload sizes, not allocator/RSS measurements.
pub fn inbound_bytes(item: &crate::framing::Inbound) -> Option<usize> {
    use crate::framing::Inbound;
    match item {
        Inbound::Frame(frame) => serde_json::to_vec(frame.as_ref())
            .ok()
            .map(|bytes| bytes.len()),
        Inbound::Data(frame) => Some(frame.payload.len()),
        Inbound::Datagram(packet) => Some(packet.frame.payload.len()),
        Inbound::Unknown(kind) => Some(kind.len()),
        Inbound::Broken(_) | Inbound::Lost(_) => Some(0),
    }
}

pub fn session_event_bytes(item: &crate::session::SessionEvent) -> Option<usize> {
    use crate::session::SessionEvent;
    match item {
        SessionEvent::Frame(frame) => serde_json::to_vec(frame.as_ref())
            .ok()
            .map(|bytes| bytes.len()),
        SessionEvent::Data(frame) => Some(frame.payload.len()),
        SessionEvent::Datagram(packet) => Some(packet.frame.payload.len()),
        SessionEvent::Ended(_) => Some(0),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "observer controls fail by panicking")]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_actual_public_probe_scope_registers_the_owned_machine_command() {
        let _serial = lock(&RECORDING_TEST_LOCK);
        let machine = orch_types::new_id();
        let request = orch_types::new_id();
        let command = orch_types::new_id();
        let recording = Recording::for_machine(machine, 1, 1).unwrap();
        issued_probe(machine, command, "machine.probe");
        assert!(
            recording.recorder.snapshot().commands.is_empty(),
            "background commands have no public request correlation"
        );
        probe_request(machine, &request.to_string(), async {
            issued_probe(orch_types::new_id(), command, "machine.probe");
            issued_probe(machine, command, "session.open");
            assert!(recording.recorder.snapshot().commands.is_empty());
            issued_probe(machine, command, "machine.probe");
        })
        .await;
        let facts = recording.recorder.snapshot();
        assert_eq!(facts.commands.len(), 1);
        assert_eq!(facts.commands[0].request_id, Some(request));
        assert_eq!(facts.commands[0].machine_id, Some(machine));
        recording
            .recorder
            .mark_machine(command, Phase::ApiSend, orch_types::new_id());
        assert_eq!(recording.recorder.snapshot().commands[0].seen, [0; 4]);
        probe_request(machine, &orch_types::new_id().to_string(), async {
            issued_probe(machine, orch_types::new_id(), "machine.probe");
        })
        .await;
        assert!(recording.recorder.snapshot().overflowed);
        assert_eq!(recording.recorder.snapshot().commands.len(), 1);
    }

    #[test]
    fn gateway_session_and_connector_identity_conflicts_stay_incomplete() {
        let _serial = lock(&RECORDING_TEST_LOCK);
        let machine = orch_types::new_id();
        let session = orch_types::new_id();
        let command = orch_types::new_id();
        let recording = Recording::for_machine(machine, 2, 1).unwrap();
        recording
            .recorder
            .register_probe(machine, command, "machine.probe", None, Some(session));
        recording.recorder.api_ack(command, machine, session);
        assert_eq!(
            recording.recorder.snapshot().commands[0].gateway_session_id,
            Some(session)
        );
        recording
            .recorder
            .api_ack(command, machine, orch_types::new_id());
        assert!(recording.recorder.snapshot().overflowed);
        machine_started(orch_types::new_id());
        assert!(recording.recorder.snapshot().overflowed);
    }

    #[test]
    fn selected_payload_is_retained_until_processing_or_cancellation_finishes() {
        let queue = QueueGauge::new("test.retained");
        queue.enqueue(Some(11));
        let first = queue.selected(Some(11));
        queue.enqueue(Some(13));
        assert_eq!(queue.snapshot().peak_retained_payload_bytes, 24);
        let second = queue.selected(Some(13));
        first.processed();
        assert_eq!(queue.snapshot().in_flight_payload_bytes, 13);
        drop(second);
        let snapshot = queue.snapshot();
        assert_eq!(
            (
                snapshot.processed,
                snapshot.sent,
                snapshot.selected_without_completion
            ),
            (1, 0, 1)
        );
        assert_eq!(snapshot.in_flight_payload_bytes, 0);
        assert!(!snapshot.inconsistent);
    }

    #[test]
    fn receiver_retirement_never_erases_loss_or_late_completion() {
        let clean = QueueGauge::new("test.retired");
        clean.enqueue(Some(7));
        clean.selected(Some(7)).processed();
        drop(clean.receiver());
        assert!(clean.snapshot().drain_or_retired());
        let retired = clean.snapshot().drained_retirement;
        clean.enqueue(Some(0)); // A later producer cannot extend the immutable close witness.
        assert_eq!(clean.snapshot().drained_retirement, retired);
        assert!(!clean.snapshot().drain_or_retired());

        let inflight = QueueGauge::new("test.inflight-close");
        inflight.enqueue(Some(7));
        let selected = inflight.selected(Some(7));
        drop(inflight.receiver());
        selected.complete();
        assert!(inflight.snapshot().conserved_empty());
        assert!(inflight.snapshot().drained_retirement.is_none());
        assert!(!inflight.snapshot().drain_or_retired());

        let lost = QueueGauge::new("test.discard-close");
        lost.enqueue(Some(7));
        drop(lost.receiver());
        assert_eq!(lost.snapshot().discarded_on_receiver_close, 1);
        assert!(lost.snapshot().drained_retirement.is_none());
        assert!(!lost.snapshot().drain_or_retired());
    }

    #[test]
    fn drain_failed_enqueue_and_incomplete_selection_are_different_observations() {
        let queue = QueueGauge::new("test.outcomes");
        queue.enqueue(Some(7));
        queue.selected(Some(7)).complete();
        queue.enqueue(Some(11));
        drop(queue.selected(Some(11))); // No conclusion about cancellation versus sink failure.
        queue.enqueue(Some(13));
        queue.failed_enqueue(Some(13));
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.enqueue_attempts, 3);
        assert_eq!(snapshot.sent, 1);
        assert_eq!(snapshot.selected_without_completion, 1);
        assert_eq!(snapshot.failed_enqueues, 1);
        assert_eq!(snapshot.queued_messages, 0);
        assert_eq!(snapshot.in_flight_messages, 0);
        assert_eq!(snapshot.discarded_on_receiver_close, 0);
        assert!(!snapshot.inconsistent && !snapshot.indeterminate_close_race);
    }

    #[test]
    fn closing_a_receiver_does_not_masquerade_as_drain_or_conservation() {
        let queue = QueueGauge::new("test.close-race");
        queue.enqueue(Some(9));
        drop(queue.receiver());
        queue.failed_enqueue(Some(9));
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.discarded_on_receiver_close, 1);
        assert!(snapshot.receiver_closed && snapshot.indeterminate_close_race);
        assert_eq!(
            snapshot.queued_messages, 0,
            "zero after closure alone is not a drain proof"
        );
    }

    #[test]
    fn the_recorder_has_explicit_command_and_queue_bounds() {
        let _serial = lock(&RECORDING_TEST_LOCK);
        let command = orch_types::new_id();
        let recording = Recording::install(&[command], 1).unwrap();
        let _first = QueueGauge::new("test.first");
        let _excess = QueueGauge::new("test.excess");
        recording.recorder.mark(command, Phase::ApiSend);
        recording.recorder.mark(command, Phase::GatewaySend);
        recording.recorder.mark(command, Phase::GatewayAck);
        recording.recorder.mark(command, Phase::ApiAck);
        recording.recorder.mark(orch_types::new_id(), Phase::ApiAck);
        let snapshot = recording.recorder.snapshot();
        assert!(
            snapshot.overflowed,
            "dropped observations cannot produce a complete proof"
        );
        assert_eq!(snapshot.queues.len(), 1);
        assert_eq!(snapshot.commands.len(), 1);
        assert_eq!(snapshot.commands[0].seen, [1, 1, 1, 1]);
        assert!(snapshot.commands[0].carrier_ack_micros.is_some());
    }

    #[tokio::test]
    async fn aborting_a_blocked_test_writer_releases_its_observed_inflight_entry() {
        let queue = QueueGauge::new("test.blocked");
        queue.enqueue(Some(17));
        let started = Arc::new(tokio::sync::Notify::new());
        let selected = Arc::clone(&started);
        let held = Arc::clone(&queue);
        let writer = tokio::spawn(async move {
            let flight = held.selected(Some(17));
            selected.notify_one();
            std::future::pending::<()>().await;
            flight.complete();
        });
        started.notified().await;
        assert_eq!(queue.snapshot().in_flight_messages, 1);
        writer.abort();
        assert!(writer.await.unwrap_err().is_cancelled());
        let snapshot = queue.snapshot();
        assert_eq!(snapshot.in_flight_messages, 0);
        assert_eq!(snapshot.selected_without_completion, 1);
        assert!(!snapshot.inconsistent);
    }
}
