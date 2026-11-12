//! What a sub-agent's requests cost, settled from the sub-agent's own transcript, and which turn's use holds them.
//!
//! The program prints each message of a sub-agent as the message is complete, and the model's closing usage of
//! it comes after: the printed message carries the output count it started with (one token), and the program
//! forwards no stream events for a sub-agent. The program's own transcript of the sub-agent
//! (`projects/<directory>/<session>/subagents/agent-<agent id>.jsonl`, in a subdirectory of `subagents` for an
//! agent a parent grouped) is written a little afterwards, with a line for each block of a message. A line that
//! the program wrote after the message ended carries its stop reason and the usage the request really had (the
//! pinned program writes every line of a finished message so); a line written before carries the count the message
//! had then and no stop reason. Counts only grow while a message is written, so of the lines that carry a stop
//! reason the one with the most output is the closing one. The engine keeps the first final report of a request,
//! so a figure reported early stays; a request is therefore held here and reported once, with the transcript's
//! figure.
//!
//! # Who does what
//!
//! The mapper only *hears* ([`Heard`]): a message printed, a result that closes a window of totals and what its
//! growth names. The conversation hands that, and the end of a turn, to a task of its own, the settler
//! ([`Settling::spawn`]), which owns the held requests, looks at the transcripts and gives its reports to the same
//! sink the conversation uses. The conversation's loop never waits for a file.
//!
//! # Which window holds a request, and what a turn states
//!
//! A request is owned by the turn that started its sub-agent, and the engine counts it as a row of that turn, whenever
//! the row comes. A turn's own use, as the program reports it, is the growth of its running totals since the result
//! before: the *window* of totals (numbered once for the conversation, whatever process or mapper serves it). The
//! program adds the cost of a message to its totals when the message ends, not when it prints it, so the window that
//! holds a request is not always the one it was printed in, and the use of a turn holds the requests of other turns'
//! sub-agents that ended in its window.
//!
//! The mapper names the requests a window's growth holds by the exact sums of the printed messages against what each
//! model's growth holds beyond the main conversation's own use ([`Placement`]); where the growth cannot be told (the
//! base of the totals is not known) it names the messages that have certainly ended. A request is a row only when its
//! window is decided:
//! - *named*: its cost is in the use of the turn that closes that window. It is reported as a row; the turn states the
//!   figure its own rows carry (`carried_by_rows`) and leaves the others' out of its use ([`leave_out`]), so that the
//!   engine, which counts every row whenever it comes, closes the turn as its use less the stated figure;
//! - *unknown*: it was decided, later, to lie in an earlier window whose base was not known, whose use holds no part
//!   of it: a row, with nothing stated or left out;
//! - *dropped*: it was never decided (the sums did not name it, and it has ended or the program is gone): no row. The
//!   growth that holds it has counted it once, in the remainder of that turn.
//!
//! A request that the sums cannot place when its window closes stays pending, and is tried again at each later result
//! by the exact match only, never by whether there is room. Nothing here depends on the order in which the rows and
//! the completion reach the engine.
//!
//! # The gate and the one budget
//!
//! The gate holds a turn's use, up to the configured wait, until the figures of the rows inside that use are final, so
//! that the stated figure and the rows agree. **Only the settler opens the gate** (or, when the settler is gone, the
//! conversation, after the settler has reported what it held): no clock of the conversation's lets a use overtake a
//! report. The profile option `child_usage_wait_ms` (a second by default, `wait` below) is the budget: a decided request
//! is reported at the latest `wait` after its window was decided, or, once the program is gone, after one last read
//! (the exit of the program is waited for, at most [`EXIT_GRACE`], and the deadlines of the requests held then are
//! moved by that much). `0` is no hold at all: the request is reported at once as the stream said it. The settler waits
//! for nothing else:
//! - a read of a transcript runs on a blocking thread and answers by message; the settler's loop does not wait for
//!   it, and a request is reported at its deadline whatever the read is at. A read that has not answered within
//!   `wait` (a disk that has stopped answering) is left to itself and another is started; its answer, if it comes,
//!   is dropped;
//! - the one wait left is the send of a report to the sink, which is the host's own back-pressure.
//!
//! # The look schedule
//!
//! A request is not looked for before its window is decided (the transcript of a message that has not ended shows
//! nothing). From then on it is looked at at once and every [`LOOK_DUE`] until its transcript shows it, or its
//! deadline. One request that never settles delays no other request's look, and is reported at its deadline.
//!
//! # Reading
//!
//! A transcript is read forward from where the last look ended, one reader for each sub-agent, never from the
//! start again; a line longer than 1 MiB is skipped unread, one look reads at most 8 MiB, and links are not
//! followed below the session's directory. A file that was replaced (another device or inode) or rewritten (its
//! first bytes changed, or it got shorter) is read from its start. The usages and the tool calls of the messages
//! are remembered (the last 4096 of a file), so that a report names the calls of the whole message, whenever its
//! blocks were heard. A reader is forgotten once nothing waits for its agent for a minute.
//!
//! # What happens when
//!
//! | Situation | What is done |
//! |---|---|
//! | The message names no sub-agent or session, a name is unusable, or the harness has no home | Reported as the stream said it, once its window is decided |
//! | A request is not shown by its deadline (no file, no directory, a read that does not answer, a line never written) | Reported as the stream said it, with a debug line |
//! | The session shows no transcripts at all when a request of it reaches its deadline | That is remembered for the session: its other decided requests are reported at once, and so are later ones, as long as the first look still finds no directory |
//! | The sub-agent is still running when its turn ends | What the window holds is named and settled; its later requests are decided at the results that follow |
//! | The program is lost | The requests decided are read once more, to the end of the files, as soon as the program has exited (never later than [`EXIT_GRACE`]), and the rest reported as the stream said it; those not decided are dropped |
//! | The settler fails | What it holds and has decided is reported as the stream said it, and the gates are opened; what it holds undecided is dropped, and so is what comes after: the ledger it kept is still used for the turn's statement |
//! | Two sub-agents at once | Each request is settled from the transcript of its own agent |
//! | A sub-agent of a sub-agent | The transcript is found by the agent's name anywhere under the session's `subagents` directory, where the program groups it |
//! | A block of a message is printed after the message was reported | Not reported again; its calls are in the report when the transcript was what settled it |
//! | A message is printed again after its request was reported as the stream said it | Ignored, deliberately: the engine keeps the first final report of a request, so a second one could add nothing but the calls the first lacked, and the engine's keeping of those is not relied on |
//!
//! What remains: a line the program never flushed (it was killed first) is not in the file, and its request is
//! reported with the output count the message started with; the ledger then keeps that figure.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::FutureExt;
use orch_agent_contract::adapter::EventSink;
use orch_agent_contract::events::{TokenUsage, UsageUpdated};
use orch_agent_contract::requests::UsageRequest;
use orch_types::{Id, Json};
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep_until};

use crate::config::{EXIT_GRACE, MANAGED_PROJECT_DIR};
use crate::history::projects_directory;
use crate::performance::{Printed, settle_usage};

/// The interval between looks for a request whose window is decided.
pub const LOOK_DUE: Duration = Duration::from_millis(25);
/// How deep below a session's `subagents` directory the transcript of a sub-agent is looked for: the program puts
/// the transcript of an agent a parent grouped into a subdirectory (a sub-agent of a sub-agent among them).
const SEARCH_DEPTH: usize = 4;
/// The longest line of a transcript that is read: a message is what the model wrote (a few thousand tokens), and a
/// line longer than this is a tool's result.
const LINE_MAX: usize = 1 << 20;
/// The most one look reads of a transcript.
const LOOK_MAX: u64 = 8 << 20;
/// How many looks of that size the last look of a transcript may make.
const FINAL_LOOKS: u64 = 8;
/// How many messages are remembered of a transcript.
const MESSAGES_MAX: usize = 4096;
/// How many reported messages the settler remembers, to report none twice.
const REPORTED_MAX: usize = 4096;
/// How long a reader is kept after its last look when no request needs it.
const READER_IDLE: Duration = Duration::from_secs(60);
/// How many bytes at the start of a transcript are kept to tell that it was rewritten.
const HEAD_BYTES: usize = 64;

/// A request is known by the tool use that started its sub-agent and by the model's message.
pub type Key = (String, String);
/// A sub-agent is known by its session and its name.
type Group = (String, String);

/// What a result's growth of the totals says of the requests that wait: the mapper's decision for one window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Placement {
    /// The window the result closed.
    pub window: u64,
    /// Whether the base the window's growth was taken from was known. Where it was not, the use of the turn holds
    /// each named message as it was printed, since the totals cannot be asked.
    pub base_known: bool,
    /// The requests whose cost is in the use of the turn that closes the window.
    pub named: Vec<Key>,
    /// The requests decided, later, to lie in an earlier window whose base was not known.
    pub unknown: Vec<Key>,
    /// The requests that are never to be decided: no row.
    pub dropped: Vec<Key>,
}

/// What the mapper heard of sub-agents, in the order it heard it.
#[derive(Debug, Clone)]
pub enum Heard {
    /// A message of a sub-agent was printed, in a turn: the request it describes waits for its window to be decided.
    Printed {
        /// The turn that started the sub-agent, which the request belongs to.
        turn: Id,
        /// The message.
        printed: Box<Printed>,
    },
    /// A result closed a window of totals, and the requests it names ([`Placement`]).
    Placed(Placement),
    /// Requests that will not be decided: the program that printed them is over, or too many wait.
    Dropped(Vec<Key>),
}

#[derive(Debug)]
struct Hold {
    segment: Id,
    turn: Id,
    printed: Printed,
}

#[derive(Debug)]
enum Note {
    Printed(Box<Hold>),
    Placed(Placement),
    Dropped(Vec<Key>),
    Gate {
        window: u64,
        gate: u64,
    },
    Crash,
    Lost {
        exited: Option<watch::Receiver<bool>>,
    },
}

/// What the settler tells itself: the answer of a read, and the exit of a program.
enum Inner {
    Read(Box<Finished>),
    Exited(u64),
}

struct Finished {
    group: Group,
    generation: u64,
    started: Instant,
    done: Option<Done>,
}

/// How the settler reads a transcript. It runs on a blocking thread; [`Files`] is the one the adapter uses, and a
/// case may stand in another to answer at once or not at all.
pub trait Transcripts: Send + Sync + 'static {
    /// What is new in the transcript of the agent `agent` of the session `session` since `reader` last looked,
    /// and where the transcript is. With `last`, the whole of what is left, up to the eight looks of a last read.
    fn look(&self, home: &Path, session: &str, agent: &str, reader: Reader, last: bool) -> Done;
}

/// The transcripts that the program keeps in its home.
#[derive(Debug, Clone, Copy, Default)]
pub struct Files;

impl Transcripts for Files {
    fn look(&self, home: &Path, session: &str, agent: &str, reader: Reader, last: bool) -> Done {
        work(home, session, agent, reader, last)
    }
}

/// What a reported request the use of a window holds: for the turn that closes the window to state (its own) or to
/// leave out (the others').
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contribution {
    /// The window of totals whose use holds it.
    pub window: u64,
    /// The turn that owns the request.
    pub owner: Id,
    /// The model, as the message named it.
    pub model: Option<String>,
    /// What the use of the window holds of it: the figure of its row where the totals held its cost, the message as
    /// it was printed where the base of the totals was not known.
    pub held: TokenUsage,
}

/// What a turn states of its use and leaves out of it, for its window ([`Settling::take_window`]).
#[derive(Debug, Clone, Default)]
pub struct Carried {
    /// What the rows of the turn's own requests carry of its use.
    pub own: TokenUsage,
    /// What the others' requests add to it, to be left out of the use ([`leave_out`]).
    pub others: Vec<Contribution>,
}

type Ledger = Arc<Mutex<Vec<Contribution>>>;

/// How the conversation talks to its settler. It is cheap to clone: the handle shares the settler.
#[derive(Debug, Clone)]
pub struct Settling {
    notes: mpsc::UnboundedSender<Note>,
    /// How many requests are named for a window and not yet reported.
    owed: Arc<AtomicUsize>,
    ledger: Ledger,
}

impl Settling {
    /// Starts the settler of a conversation. It reports to `sink` for `thread`; `home` is where the program keeps
    /// its transcripts; `wait` is the budget of the module. The numbers it sends back are the gates that are
    /// open: a gate is opened when everything it waited for has been reported.
    #[must_use]
    pub fn spawn(
        sink: EventSink,
        thread: Id,
        home: Option<PathBuf>,
        wait: Duration,
    ) -> (Self, mpsc::UnboundedReceiver<u64>) {
        Self::spawn_with(sink, thread, home, wait, Arc::new(Files))
    }

    /// As [`Settling::spawn`], reading the transcripts by `files`.
    #[must_use]
    pub fn spawn_with(
        sink: EventSink,
        thread: Id,
        home: Option<PathBuf>,
        wait: Duration,
        files: Arc<dyn Transcripts>,
    ) -> (Self, mpsc::UnboundedReceiver<u64>) {
        let (notes, received) = mpsc::unbounded_channel();
        let (inner, inner_received) = mpsc::unbounded_channel();
        let (gates_sent, gates) = mpsc::unbounded_channel();
        let owed = Arc::new(AtomicUsize::new(0));
        let ledger = Ledger::default();
        let settler = Settler {
            ledger: Arc::clone(&ledger),
            sink,
            thread,
            home,
            wait,
            files,
            owed: Arc::clone(&owed),
            gates: gates_sent,
            inner,
            items: HashMap::new(),
            reported: VecDeque::new(),
            watches: HashMap::new(),
            none: HashSet::new(),
            waiters: Vec::new(),
            epochs: 0,
            closing: false,
        };
        tokio::spawn(supervise(settler, received, inner_received));
        (
            Self {
                notes,
                owed,
                ledger,
            },
            gates,
        )
    }

    /// Whether a request that is named for a window may still be unreported: the use of a turn is not made up before
    /// it is.
    #[must_use]
    pub fn owes(&self) -> bool {
        self.owed.load(Ordering::SeqCst) > 0
    }

    /// Whether the settler has failed or is over: nothing is held any longer, and what is printed is not reported.
    #[must_use]
    pub fn is_gone(&self) -> bool {
        self.notes.is_closed()
    }

    /// A message of a sub-agent was printed: its request waits for the window it is in to be decided. Where the
    /// settler is gone nothing waits, and the request is not reported: the use of the turn that holds it counts it.
    pub fn printed(&self, segment: Id, turn: Id, printed: Printed) {
        if self
            .notes
            .send(Note::Printed(Box::new(Hold {
                segment,
                turn,
                printed,
            })))
            .is_err()
        {
            tracing::debug!(
                "a sub-agent's message was printed after the settler had gone, and is not reported"
            );
        }
    }

    /// A result closed a window, and the mapper says which requests it holds. What is named is owed at once, before the
    /// settler takes the note up, so that a turn that ends now waits for it.
    pub fn placed(&self, placement: Placement) {
        let named = placement.named.len();
        self.owed.fetch_add(named, Ordering::SeqCst);
        if self.notes.send(Note::Placed(placement)).is_err() {
            self.owed.fetch_sub(named, Ordering::SeqCst);
        }
    }

    /// Requests that will not be decided: no row is reported for them.
    pub fn dropped(&self, keys: Vec<Key>) {
        let _ = self.notes.send(Note::Dropped(keys));
    }

    /// A turn that ends waits behind the gate numbered `gate` for the requests named for its window, and the ones
    /// named for the windows before. The number comes back when all of them are reported.
    pub fn gate(&self, window: u64, gate: u64) {
        let _ = self.notes.send(Note::Gate { window, gate });
    }

    /// What the turn `turn`, whose result closed `window`, states of its use and leaves out of it: the figure the
    /// rows of its own requests carry, and the others' contributions. What was reported for this window and the ones
    /// before is forgotten.
    #[must_use]
    pub fn take_window(&self, window: u64, turn: Id) -> Carried {
        let mut ledger = self
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut carried = Carried::default();
        ledger.retain(|contribution| {
            if contribution.window == window {
                if contribution.owner == turn {
                    carried.own = carried.own.plus(contribution.held);
                } else {
                    carried.others.push(contribution.clone());
                }
            }
            contribution.window > window
        });
        carried
    }

    /// Makes the settler fail, so that a case can see what is done when it does. Nothing else calls it.
    #[doc(hidden)]
    pub fn crash(&self) {
        let _ = self.notes.send(Note::Crash);
    }

    /// The program is gone, or is going once `exited` says so: every request decided is looked for a last time, once
    /// the program has exited, and reported; the others are dropped.
    pub fn lost(&self, exited: Option<watch::Receiver<bool>>) {
        let _ = self.notes.send(Note::Lost { exited });
    }
}

/// Enters what the use of `window` holds of a request in the ledger; those of one window, owner and model are summed.
fn note_contribution(
    ledger: &Ledger,
    window: u64,
    owner: Id,
    model: Option<String>,
    held: TokenUsage,
) {
    let mut ledger = ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match ledger.iter_mut().find(|each| {
        each.window == window && each.owner == owner && each.model == model
    }) {
        Some(sum) => sum.held = sum.held.plus(held),
        None => ledger.push(Contribution {
            window,
            owner,
            model,
            held,
        }),
    }
}

/// How far a held request has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    /// Its window is not decided: it is not looked for, and no row is reported for it.
    Undecided,
    /// Its cost is in the use of the turn that closed this window.
    Window(u64),
    /// It lies in an earlier window whose base was not known: a row, with nothing stated.
    Unknown,
}

#[derive(Debug)]
struct Item {
    request: UsageRequest,
    turn: Id,
    fate: Fate,
    /// What the use holds of it, where that is not its row's figure: the message as it was printed.
    held: Option<TokenUsage>,
    segment: Id,
    agent: String,
    session: String,
    /// Why it cannot be looked for at all (the message names no agent, or an unusable name, or the harness has no
    /// home): it is reported as the stream said it once its window is decided.
    unreadable: Option<&'static str>,
    /// When it is reported as the stream said it if the transcript does not show it; none while it is undecided.
    deadline: Option<Instant>,
    next_look: Instant,
    /// Its session was found to have no transcripts: it is reported at once while the first look finds none.
    quick: bool,
    /// The program is stopping (the number of that stop): the last read waits for its exit.
    exit: Option<u64>,
    /// The program has exited: the first read that begins after this is the last.
    last_since: Option<Instant>,
}

impl Item {
    fn decided(&self) -> bool {
        self.fate != Fate::Undecided
    }

    /// Decides the item: it is looked for at once, and reported within `wait` at the latest (at once where there is
    /// no wait).
    fn decide(&mut self, fate: Fate, now: Instant, wait: Duration) {
        self.fate = fate;
        // What cannot be looked for is not waited for.
        self.deadline = Some(if self.unreadable.is_some() { now } else { now + wait });
        self.next_look = now;
    }
}

#[derive(Debug)]
enum State {
    Held(Box<Item>),
    Reported,
}

#[derive(Debug)]
struct Waiter {
    gate: u64,
    /// The requests the gate waits for: those named for the window, or an earlier one, that were held when the turn
    /// ended.
    keys: Vec<Key>,
}

#[derive(Debug, Clone, Copy)]
struct Reading {
    started: Instant,
}

/// What is known of one sub-agent's transcript.
#[derive(Debug)]
struct Watch {
    reader: Reader,
    /// A read is out, and since when.
    reading: Option<Reading>,
    /// Which read is the one whose answer counts: one that was left is dropped when it answers.
    generation: u64,
    /// What the last read that answered found.
    located: Located,
    used: Instant,
}

impl Watch {
    fn new(now: Instant) -> Self {
        Self {
            reader: Reader::default(),
            reading: None,
            generation: 0,
            located: Located::NoFile,
            used: now,
        }
    }
}

struct Settler {
    sink: EventSink,
    thread: Id,
    home: Option<PathBuf>,
    wait: Duration,
    files: Arc<dyn Transcripts>,
    ledger: Ledger,
    owed: Arc<AtomicUsize>,
    gates: mpsc::UnboundedSender<u64>,
    inner: mpsc::UnboundedSender<Inner>,
    items: HashMap<Key, State>,
    reported: VecDeque<Key>,
    watches: HashMap<Group, Watch>,
    none: HashSet<String>,
    waiters: Vec<Waiter>,
    epochs: u64,
    closing: bool,
}

/// Runs the settler; if it fails, what it holds is reported as the streams said it and the gates are opened, so
/// that nothing waits for a task that is gone.
async fn supervise(
    mut settler: Settler,
    mut notes: mpsc::UnboundedReceiver<Note>,
    mut inner: mpsc::UnboundedReceiver<Inner>,
) {
    let outcome = AssertUnwindSafe(settler.run(&mut notes, &mut inner))
        .catch_unwind()
        .await;
    if outcome.is_err() {
        tracing::error!(
            "the settler of sub-agent requests failed; what it holds is reported as the streams said it"
        );
        // What the conversation sent before the failure and the settler had not taken up is taken up now, so that
        // what was named is reported and the rest dropped.
        notes.close();
        while let Ok(note) = notes.try_recv() {
            settler.hear_after_failure(note);
        }
        settler.release_all().await;
    }
}

async fn sleep_until_or_pending(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => std::future::pending::<()>().await,
    }
}

fn usable(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && !name.contains("..")
}

impl Settler {
    async fn run(
        &mut self,
        notes: &mut mpsc::UnboundedReceiver<Note>,
        inner: &mut mpsc::UnboundedReceiver<Inner>,
    ) {
        loop {
            if self.closing && !self.any_held() {
                return;
            }
            let wake = self.next_wake();
            tokio::select! {
                note = notes.recv(), if !self.closing => match note {
                    Some(note) => self.hear(note),
                    None => self.close(),
                },
                Some(event) = inner.recv() => match event {
                    Inner::Read(finished) => self.read_done(*finished).await,
                    Inner::Exited(epoch) => self.finish(Some(epoch)),
                },
                () = sleep_until_or_pending(wake) => {}
            }
            self.tick().await;
        }
    }

    fn any_held(&self) -> bool {
        self.items
            .values()
            .any(|state| matches!(state, State::Held(_)))
    }

    /// How long a read may be out before another is started in its place: the budget.
    fn stuck(&self) -> Duration {
        self.wait.max(LOOK_DUE)
    }

    fn next_wake(&self) -> Option<Instant> {
        let stuck = self.stuck();
        self.items
            .values()
            .filter_map(|state| match state {
                State::Held(item) if item.decided() => {
                    let group = (item.session.clone(), item.agent.clone());
                    let look = match self
                        .watches
                        .get(&group)
                        .and_then(|watch| watch.reading.as_ref())
                    {
                        // The answer of the read that is out is awaited as a message; only its being left is timed.
                        Some(reading) => reading.started + stuck,
                        None => item.next_look,
                    };
                    Some(item.deadline.map_or(look, |deadline| deadline.min(look)))
                }
                _ => None,
            })
            .min()
    }

    fn hear(&mut self, note: Note) {
        let now = Instant::now();
        match note {
            Note::Printed(hold) => {
                let Hold {
                    segment,
                    turn,
                    printed,
                } = *hold;
                self.printed(segment, turn, printed);
            }
            Note::Placed(placement) => self.placed(placement, now),
            Note::Dropped(keys) => self.drop_undecided(&keys),
            Note::Crash => panic!("the settler was made to fail"),
            Note::Gate { window, gate } => {
                let keys = self
                    .items
                    .iter()
                    .filter(|(_, state)| {
                        matches!(state, State::Held(item)
                            if matches!(item.fate, Fate::Window(of) if of <= window))
                    })
                    .map(|(key, _)| key.clone())
                    .collect();
                self.waiters.push(Waiter { gate, keys });
            }
            Note::Lost { exited } => self.lost(exited, now),
        }
    }

    /// Takes up what the conversation sent before the settler failed.
    fn hear_after_failure(&mut self, note: Note) {
        match note {
            Note::Printed(hold) => {
                let Hold {
                    segment,
                    turn,
                    printed,
                } = *hold;
                self.printed(segment, turn, printed);
            }
            Note::Placed(placement) => self.placed(placement, Instant::now()),
            Note::Dropped(keys) => self.drop_undecided(&keys),
            Note::Gate { .. } | Note::Crash | Note::Lost { .. } => {}
        }
    }

    /// The mapper's decision for a window: what is named is looked for and reported within the budget, what is
    /// dropped is forgotten.
    fn placed(&mut self, placement: Placement, now: Instant) {
        let wait = self.wait;
        let Placement {
            window,
            base_known,
            named,
            unknown,
            dropped,
        } = placement;
        let mut missing = 0_usize;
        for key in named {
            match self.items.get_mut(&key) {
                Some(State::Held(item)) if !item.decided() => {
                    if !base_known {
                        item.held = Some(item.request.tokens.usage());
                    }
                    item.decide(Fate::Window(window), now, wait);
                }
                _ => missing += 1,
            }
        }
        if missing > 0 {
            self.owed.fetch_sub(missing, Ordering::SeqCst);
        }
        for key in unknown {
            if let Some(State::Held(item)) = self.items.get_mut(&key)
                && !item.decided()
            {
                item.decide(Fate::Unknown, now, wait);
            }
        }
        self.drop_undecided(&dropped);
    }

    /// Forgets the requests that are not decided and will not be: no row is reported for them.
    fn drop_undecided(&mut self, keys: &[Key]) {
        for key in keys {
            if matches!(self.items.get(key), Some(State::Held(item)) if !item.decided()) {
                tracing::debug!(
                    message = %key.1,
                    "a sub-agent's request was never placed in a window of totals and is not reported"
                );
                self.mark_reported(key.clone());
            }
        }
    }

    fn printed(&mut self, segment: Id, turn: Id, printed: Printed) {
        let Printed {
            request,
            parent,
            agent_id,
            session_id,
            message_id,
        } = printed;
        let key = (parent, message_id);
        let now = Instant::now();
        match self.items.get_mut(&key) {
            Some(State::Held(item)) => {
                // Another block of a message that waits: it names more tool calls.
                merge_refs(&mut item.request, &request);
                return;
            }
            Some(State::Reported) => {
                tracing::debug!(
                    request = %request.id,
                    "a sub-agent's message was printed again after its request was reported"
                );
                return;
            }
            None => {}
        }
        let unreadable = match (&self.home, &agent_id, &session_id) {
            (None, ..) => Some("the harness has no home to read"),
            (_, None, _) | (_, _, None) => Some("the message names no sub-agent or no session"),
            (_, Some(agent), Some(session))
                if !usable(agent) || !usable(session) || key.1.is_empty() =>
            {
                Some("the names of the request are not usable")
            }
            _ => None,
        };
        let agent = agent_id.unwrap_or_default();
        let session = session_id.unwrap_or_default();
        let quick = unreadable.is_none() && self.none.contains(&session);
        self.items.insert(
            key,
            State::Held(Box::new(Item {
                request,
                turn,
                fate: Fate::Undecided,
                held: None,
                segment,
                agent,
                session,
                unreadable,
                deadline: None,
                next_look: now,
                quick,
                exit: None,
                last_since: None,
            })),
        );
    }

    /// The program is gone, or is going once it has exited: the requests decided are read one last time then, and
    /// the others are dropped.
    fn lost(&mut self, exited: Option<watch::Receiver<bool>>, now: Instant) {
        let undecided: Vec<Key> = self
            .items
            .iter()
            .filter_map(|(key, state)| match state {
                State::Held(item) if !item.decided() => Some(key.clone()),
                _ => None,
            })
            .collect();
        self.drop_undecided(&undecided);
        let Some(exited) = exited else {
            self.finish(None);
            return;
        };
        self.epochs += 1;
        let epoch = self.epochs;
        let wait = self.wait;
        for state in self.items.values_mut() {
            if let State::Held(item) = state
                && item.last_since.is_none()
            {
                item.exit = Some(epoch);
                item.deadline = Some(
                    item.deadline
                        .unwrap_or(now)
                        .max(now + EXIT_GRACE + wait),
                );
            }
        }
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _ = tokio::time::timeout(EXIT_GRACE, exit_wait(exited)).await;
            let _ = inner.send(Inner::Exited(epoch));
        });
    }

    /// The program has exited (`Some`: the one that was waited for; `None`: every request held): what is held is
    /// read to the end, and reported as soon as that read has answered.
    fn finish(&mut self, epoch: Option<u64>) {
        let now = Instant::now();
        let wait = self.wait;
        for state in self.items.values_mut() {
            if let State::Held(item) = state
                && item.decided()
                && (epoch.is_none() || item.exit == epoch)
            {
                item.exit = None;
                item.last_since = Some(now);
                item.next_look = now;
                item.deadline = Some(now + wait);
            }
        }
    }

    /// The conversation is over: what is decided is read to the end and reported; the rest is dropped.
    fn close(&mut self) {
        self.closing = true;
        let undecided: Vec<Key> = self
            .items
            .iter()
            .filter_map(|(key, state)| match state {
                State::Held(item) if !item.decided() => Some(key.clone()),
                _ => None,
            })
            .collect();
        self.drop_undecided(&undecided);
        self.finish(None);
    }

    /// One pass: what has run out of its time is reported, the reads that are due are started, the gates whose
    /// requests are reported are opened, and what is no longer needed is forgotten.
    async fn tick(&mut self) {
        let now = Instant::now();
        self.expire(now).await;
        self.start_reads(now);
        self.finish_waiters();
        self.tidy(now);
    }

    /// Reports, as the stream said it, the requests whose deadline has passed.
    async fn expire(&mut self, now: Instant) {
        let late: Vec<Key> = self
            .items
            .iter()
            .filter_map(|(key, state)| match state {
                State::Held(item) if item.deadline.is_some_and(|deadline| deadline <= now) => {
                    Some(key.clone())
                }
                _ => None,
            })
            .collect();
        for key in late {
            let Some(State::Held(item)) = self.items.get(&key) else {
                continue;
            };
            let session = item.session.clone();
            let group = (item.session.clone(), item.agent.clone());
            let unreadable = item.unreadable;
            let (located, reading) = self
                .watches
                .get(&group)
                .map_or((Located::NoFile, false), |watch| {
                    (watch.located.clone(), watch.reading.is_some())
                });
            let why = match &located {
                _ if unreadable.is_some() => unreadable.unwrap_or_default(),
                Located::NoDirectory => "the session shows no transcripts",
                _ if reading => "the transcript could not be read in time",
                Located::NoFile => "the transcript of the sub-agent is not there",
                Located::File(_) => "the transcript does not show the request finished",
            };
            self.report(&key, None, Some(why)).await;
            if unreadable.is_none() && located == Located::NoDirectory {
                self.no_transcripts(session).await;
            }
        }
    }

    /// Starts a read for every sub-agent that has a request whose time has come, if none is out.
    fn start_reads(&mut self, now: Instant) {
        let Some(home) = self.home.clone() else {
            return;
        };
        let mut wanted: HashMap<Group, bool> = HashMap::new();
        for state in self.items.values() {
            if let State::Held(item) = state
                && item.decided()
                && item.unreadable.is_none()
            {
                let ripe = item.next_look <= now || item.last_since.is_some();
                let entry = wanted
                    .entry((item.session.clone(), item.agent.clone()))
                    .or_insert(false);
                if ripe {
                    *entry = true;
                }
            }
        }
        let stuck = self.stuck();
        for (group, ripe) in wanted {
            if !ripe {
                continue;
            }
            let last = self.items.values().any(|state| {
                matches!(state, State::Held(item)
                    if item.last_since.is_some()
                        && item.session == group.0
                        && item.agent == group.1)
            });
            let watch = self
                .watches
                .entry(group.clone())
                .or_insert_with(|| Watch::new(now));
            if let Some(reading) = watch.reading {
                if now.saturating_duration_since(reading.started) < stuck {
                    continue;
                }
                // The read that is out has not answered within the budget: it is left, and its answer dropped.
                watch.reader = Reader::default();
            }
            watch.generation += 1;
            let generation = watch.generation;
            watch.reading = Some(Reading { started: now });
            let reader = std::mem::take(&mut watch.reader);
            let files = Arc::clone(&self.files);
            let inner = self.inner.clone();
            let home = home.clone();
            tokio::task::spawn_blocking(move || {
                let (session, agent) = group;
                let done = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    files.look(&home, &session, &agent, reader, last)
                }))
                .ok();
                let _ = inner.send(Inner::Read(Box::new(Finished {
                    group: (session, agent),
                    generation,
                    started: now,
                    done,
                })));
            });
        }
    }

    /// A read has answered: the requests of its sub-agent that it shows are reported, and those it does not show
    /// are looked for again later.
    async fn read_done(&mut self, finished: Finished) {
        let Finished {
            group,
            generation,
            started,
            done,
        } = finished;
        let now = Instant::now();
        let keys: Vec<Key> = self
            .items
            .iter()
            .filter_map(|(key, state)| match state {
                State::Held(item)
                    if item.decided()
                        && item.unreadable.is_none()
                        && item.session == group.0
                        && item.agent == group.1 =>
                {
                    Some(key.clone())
                }
                _ => None,
            })
            .collect();
        let Some(watch) = self.watches.get_mut(&group) else {
            return;
        };
        if watch.generation != generation {
            return;
        }
        watch.reading = None;
        watch.used = now;
        match done {
            Some(Done { reader, located }) => {
                watch.reader = reader;
                watch.located = located;
            }
            None => {
                watch.reader = Reader::default();
                watch.located = Located::NoFile;
            }
        }
        let located = watch.located.clone();
        let shown: HashMap<Key, Seen> = keys
            .iter()
            .filter_map(|key| {
                watch
                    .reader
                    .messages
                    .get(&key.1)
                    .filter(|seen| seen.usage.is_some())
                    .map(|seen| (key.clone(), seen.clone()))
            })
            .collect();
        for key in keys {
            match shown.get(&key) {
                Some(seen) => self.report_seen(&key, seen).await,
                None => self.missed(&key, &located, started, now).await,
            }
        }
    }

    /// A read did not show the request: it is looked for again later, unless the read was the last one.
    async fn missed(&mut self, key: &Key, located: &Located, started: Instant, now: Instant) {
        let Some(State::Held(item)) = self.items.get_mut(key) else {
            return;
        };
        if item.last_since.is_some_and(|since| started >= since) {
            let session = item.session.clone();
            self.report(
                key,
                None,
                Some("the transcript does not show the request after the program ended"),
            )
            .await;
            if matches!(located, Located::NoDirectory) {
                self.no_transcripts(session).await;
            }
            return;
        }
        if item.quick {
            if matches!(located, Located::NoDirectory) {
                self.report(key, None, Some("the session shows no transcripts"))
                    .await;
                return;
            }
            // The directory is there again: the session is looked for as any other.
            item.quick = false;
            self.none.remove(&item.session);
        }
        if item.next_look > now {
            // Read for the sake of another request of the agent: this one keeps its own schedule.
            return;
        }
        item.next_look = item
            .deadline
            .map_or(now + LOOK_DUE, |deadline| (now + LOOK_DUE).min(deadline));
    }

    /// The session has no transcripts: what it still holds is reported at once, and nothing waits for it.
    async fn no_transcripts(&mut self, session: String) {
        let keys: Vec<Key> = self
            .items
            .iter()
            .filter_map(|(key, state)| match state {
                State::Held(item) if item.decided() && item.session == session => {
                    Some(key.clone())
                }
                _ => None,
            })
            .collect();
        if self.none.len() >= REPORTED_MAX {
            self.none.clear();
        }
        self.none.insert(session);
        for key in keys {
            self.report(&key, None, Some("the session shows no transcripts"))
                .await;
        }
    }

    /// Reports a held request, once, with what its transcript shows of the message.
    async fn report_seen(&mut self, key: &Key, seen: &Seen) {
        let item = match self.items.remove(key) {
            Some(State::Held(item)) => item,
            Some(other) => {
                self.items.insert(key.clone(), other);
                return;
            }
            None => return,
        };
        self.mark_reported(key.clone());
        let mut item = *item;
        for call in &seen.refs {
            let known = item.request.tool_refs.get_or_insert_with(Vec::new);
            if !known.contains(call) {
                known.push(call.clone());
            }
        }
        self.send(item, seen.usage.as_ref(), None).await;
    }

    /// Reports a held request, once, as the stream said it.
    async fn report(&mut self, key: &Key, usage: Option<&Json>, why: Option<&'static str>) {
        let item = match self.items.remove(key) {
            Some(State::Held(item)) => item,
            Some(other) => {
                self.items.insert(key.clone(), other);
                return;
            }
            None => return,
        };
        self.mark_reported(key.clone());
        self.send(*item, usage, why).await;
    }

    /// Sends a request to the host. What the use of its window holds of it is entered in the ledger first, so that
    /// the turn that closes the window states and leaves out what is right when its gate opens.
    async fn send(&mut self, item: Item, usage: Option<&Json>, why: Option<&'static str>) {
        let Item {
            mut request,
            turn,
            fate,
            held,
            segment,
            ..
        } = item;
        if let Some(usage) = usage {
            settle_usage(&mut request, usage);
        } else if let Some(why) = why {
            tracing::debug!(
                request = %request.id,
                why,
                "a sub-agent's request is reported as its stream said it"
            );
        }
        // A report the engine refuses is no row: what the window's use holds of it stays in the use, in the remainder of
        // the turn that closes the window.
        if let Fate::Window(window) = fate
            && request.check().is_ok()
        {
            let held = held.unwrap_or_else(|| request.tokens.usage());
            note_contribution(&self.ledger, window, turn, request.model.clone(), held);
        }
        let _ = self.sink.request(self.thread, segment, turn, request).await;
        if matches!(fate, Fate::Window(_)) {
            self.owed.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn mark_reported(&mut self, key: Key) {
        self.items.insert(key.clone(), State::Reported);
        self.reported.push_back(key);
        while self.reported.len() > REPORTED_MAX {
            if let Some(old) = self.reported.pop_front()
                && matches!(self.items.get(&old), Some(State::Reported))
            {
                self.items.remove(&old);
            }
        }
    }

    /// Opens the gates whose requests are all reported.
    fn finish_waiters(&mut self) {
        let items = &self.items;
        let mut opened = Vec::new();
        self.waiters.retain(|waiter| {
            // Only what was held when the turn ended is waited for: a request named later is not.
            let pending = waiter
                .keys
                .iter()
                .any(|key| matches!(items.get(key), Some(State::Held(_))));
            if !pending {
                opened.push(waiter.gate);
            }
            pending
        });
        for gate in opened {
            let _ = self.gates.send(gate);
        }
    }

    /// Forgets the readers that nothing has needed for a while.
    fn tidy(&mut self, now: Instant) {
        let held: HashSet<Group> = self
            .items
            .values()
            .filter_map(|state| match state {
                State::Held(item) if item.decided() => {
                    Some((item.session.clone(), item.agent.clone()))
                }
                _ => None,
            })
            .collect();
        self.watches.retain(|group, watch| {
            held.contains(group) || watch.reading.is_some() || watch.used + READER_IDLE > now
        });
    }

    /// The settler failed: what it holds and has decided is reported as the streams said it, and nothing waits for it.
    async fn release_all(&mut self) {
        let keys: Vec<Key> = self
            .items
            .iter()
            .filter(|(_, state)| matches!(state, State::Held(item) if item.decided()))
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            self.report(&key, None, Some("the settler failed")).await;
        }
        self.owed.store(0, Ordering::SeqCst);
        for waiter in std::mem::take(&mut self.waiters) {
            let _ = self.gates.send(waiter.gate);
        }
    }
}

/// Returns when the process has exited (or is gone).
async fn exit_wait(mut exited: watch::Receiver<bool>) {
    while !*exited.borrow_and_update() {
        if exited.changed().await.is_err() {
            return;
        }
    }
}

/// Leaves out of the use of a turn what the requests of other turns' sub-agents added to the totals it grew from.
///
/// A turn's use is the growth of the program's running totals since the result before, so it holds what a
/// sub-agent started in an earlier turn used while this one ran (or between the two). The turn that owns the request
/// counts it as a row, and the engine adds the rows whenever they come: left in, it would be counted twice. What is
/// left out is what the use holds of the request, which is the figure of its row where the totals held its cost.
/// The thread's own total is the growth of the totals as it is, and stays so.
///
/// Each part is taken out by itself (the input, its cache parts, the output, the reasoning). A row that carries no
/// reasoning leaves none out, and the reasoning of a turn is a part of its output, so what the output that is left
/// cannot hold is dropped: that reasoning is the request's own, which no row could carry (the transcript does not
/// count it apart), and the total, input and output, is conserved whatever the parts.
pub fn leave_out(usage: &mut UsageUpdated, others: &[Contribution]) {
    for other in others {
        usage.turn = crate::events::less(usage.turn, other.held);
        let Some(report) = usage.report.as_mut() else {
            continue;
        };
        // The model the message named, or one that names it with a suffix (the larger window), or else the one the
        // turn used most.
        let named = other.model.as_deref().and_then(|name| {
            report
                .models
                .iter()
                .position(|entry| entry.model == name)
                .or_else(|| {
                    report.models.iter().position(|entry| {
                        entry.model.starts_with(name) || name.starts_with(&entry.model)
                    })
                })
        });
        let at = named.or_else(|| {
            report
                .models
                .iter()
                .enumerate()
                .max_by_key(|(_, entry)| entry.tokens.total_tokens)
                .map(|(at, _)| at)
        });
        if let Some(entry) = at.and_then(|at| report.models.get_mut(at)) {
            entry.tokens = crate::events::less(entry.tokens, other.held);
        }
    }
}

fn merge_refs(into: &mut UsageRequest, from: &UsageRequest) {
    let known = into.tool_refs.get_or_insert_with(Vec::new);
    for call in from.tool_refs.iter().flatten() {
        if !known.contains(call) {
            known.push(call.clone());
        }
    }
}

// ---- the files

/// Where a sub-agent's transcript is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    /// The file.
    File(PathBuf),
    /// The session has a `subagents` directory, and the agent's file is not in it (yet).
    NoFile,
    /// The session has no `subagents` directory.
    NoDirectory,
}

fn real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// The `subagents` directories of a session in a home, wherever its working directory put it. The session's
/// directory and below are real directories: a link there is not followed.
fn subagent_directories(home: &Path, session: &str) -> Vec<PathBuf> {
    if !usable(session) {
        return Vec::new();
    }
    let projects = projects_directory(home);
    let mut directories = vec![projects.join(MANAGED_PROJECT_DIR)];
    if let Ok(entries) = std::fs::read_dir(&projects) {
        directories.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
    }
    directories.sort();
    directories.dedup();
    directories
        .into_iter()
        .filter(|project| real_dir(project))
        .map(|project| project.join(session))
        .filter(|directory| real_dir(directory))
        .map(|directory| directory.join("subagents"))
        .filter(|subagents| real_dir(subagents))
        .collect()
}

fn find_named(directory: &Path, name: &str, depth: usize) -> Option<PathBuf> {
    let direct = directory.join(name);
    if std::fs::symlink_metadata(&direct).is_ok_and(|meta| meta.is_file()) {
        return Some(direct);
    }
    if depth == 0 {
        return None;
    }
    std::fs::read_dir(directory)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .find_map(|entry| find_named(&entry.path(), name, depth - 1))
}

/// The transcript of the sub-agent `agent` of the session `session` in a home.
#[must_use]
pub fn locate(home: &Path, session: &str, agent: &str, hint: Option<&Path>) -> Located {
    if !usable(session) || !usable(agent) {
        return Located::NoDirectory;
    }
    if let Some(hint) = hint
        && std::fs::symlink_metadata(hint).is_ok_and(|meta| meta.is_file())
    {
        return Located::File(hint.to_path_buf());
    }
    let name = format!("agent-{agent}.jsonl");
    let directories = subagent_directories(home, session);
    if directories.is_empty() {
        return Located::NoDirectory;
    }
    directories
        .iter()
        .find_map(|directory| find_named(directory, &name, SEARCH_DEPTH))
        .map_or(Located::NoFile, Located::File)
}

#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn identity(_: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// What a transcript shows of one model message.
#[derive(Debug, Clone, Default)]
struct Seen {
    /// The usage of the line that carries the stop reason: the usage the model closed the message with.
    usage: Option<Json>,
    /// The ids of the tool calls of all the blocks of the message.
    refs: Vec<String>,
}

/// Where a transcript was read to, and what it showed.
#[derive(Debug, Default)]
pub struct Reader {
    path: Option<PathBuf>,
    /// The device and the inode the file had when it was read.
    ident: Option<(u64, u64)>,
    /// The first bytes of the file as they were read.
    head: Vec<u8>,
    offset: u64,
    tail: Vec<u8>,
    skipping: bool,
    messages: HashMap<String, Seen>,
    order: VecDeque<String>,
}

impl Reader {
    fn at(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            ..Self::default()
        }
    }

    /// Reads forward from where the last look ended, at most `cap` bytes; whether the end of the file was reached.
    /// A file that is not the one that was read (another inode, other first bytes, shorter) is read from its start.
    fn read(&mut self, cap: u64) -> std::io::Result<bool> {
        let Some(path) = self.path.clone() else {
            return Ok(true);
        };
        let mut file = std::fs::File::open(&path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(std::io::Error::other("the transcript is not a file"));
        }
        let ident = identity(&meta);
        if meta.len() < self.offset || self.ident.is_some_and(|known| Some(known) != ident) {
            *self = Self::at(path.clone());
        }
        if self.offset > 0 && !self.head.is_empty() {
            let mut now_head = vec![0_u8; self.head.len()];
            file.read_exact(&mut now_head)?;
            if now_head != self.head {
                *self = Self::at(path);
            }
        }
        self.ident = ident;
        file.seek(SeekFrom::Start(self.offset))?;
        let at_start = self.offset == 0;
        let mut buffer = vec![0_u8; 64 * 1024];
        let mut total = 0_u64;
        loop {
            if total >= cap {
                return Ok(false);
            }
            let count = file.read(&mut buffer)?;
            if count == 0 {
                return Ok(true);
            }
            if at_start && total == 0 {
                self.head = buffer[..count.min(HEAD_BYTES)].to_vec();
            }
            total += count as u64;
            self.offset += count as u64;
            self.feed(&buffer[..count]);
        }
    }

    fn feed(&mut self, mut chunk: &[u8]) {
        while let Some(at) = chunk.iter().position(|byte| *byte == b'\n') {
            let (line, rest) = chunk.split_at(at);
            self.push(line);
            self.end_line();
            chunk = &rest[1..];
        }
        self.push(chunk);
    }

    fn push(&mut self, part: &[u8]) {
        if self.skipping {
            return;
        }
        if self.tail.len() + part.len() > LINE_MAX {
            self.skipping = true;
            self.tail = Vec::new();
            return;
        }
        self.tail.extend_from_slice(part);
    }

    fn end_line(&mut self) {
        if self.skipping {
            self.skipping = false;
            self.tail = Vec::new();
            return;
        }
        let line = std::mem::take(&mut self.tail);
        self.note(&line);
    }

    /// A line of a message names the calls of its block; a line that carries the stop reason also carries the usage
    /// the model closed the message with, while a line written before the message ended carries the count it had
    /// then. Of the lines with a stop reason the one with the most output is kept (the first of equals is replaced by
    /// a later one): the counts of a message only grow, so no line is closing that has less than another.
    fn note(&mut self, line: &[u8]) {
        let Ok(text) = std::str::from_utf8(line) else {
            return;
        };
        if !text.contains("\"message\"") || !text.contains("usage") {
            return;
        }
        let Ok(entry) = serde_json::from_str::<Json>(text) else {
            return;
        };
        let message = &entry["message"];
        let Some(id) = message["id"].as_str() else {
            return;
        };
        let refs: Vec<String> = message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|block| block["type"] == "tool_use")
            .filter_map(|block| block["id"].as_str())
            .map(str::to_owned)
            .collect();
        let closing = message["stop_reason"]
            .as_str()
            .filter(|_| message["usage"].is_object())
            .map(|_| message["usage"].clone());
        if refs.is_empty() && closing.is_none() {
            return;
        }
        if !self.messages.contains_key(id) {
            self.order.push_back(id.to_owned());
            if self.order.len() > MESSAGES_MAX
                && let Some(old) = self.order.pop_front()
            {
                self.messages.remove(&old);
            }
        }
        let seen = self.messages.entry(id.to_owned()).or_default();
        for call in refs {
            if !seen.refs.contains(&call) {
                seen.refs.push(call);
            }
        }
        if let Some(closing) = closing {
            let output = |usage: &Json| usage["output_tokens"].as_i64().unwrap_or(0);
            if seen
                .usage
                .as_ref()
                .is_none_or(|known| output(&closing) >= output(known))
            {
                seen.usage = Some(closing);
            }
        }
    }
}

/// What a look found: the reader, where it stands now, and where the transcript is.
pub struct Done {
    reader: Reader,
    located: Located,
}

/// The blocking part of a look: finds the transcript and reads what is new in it.
fn work(home: &Path, session: &str, agent: &str, mut reader: Reader, last: bool) -> Done {
    let located = locate(home, session, agent, reader.path.as_deref());
    if let Located::File(path) = &located {
        if reader.path.as_deref() != Some(path.as_path()) {
            reader = Reader::at(path.clone());
        }
        let looks = if last { FINAL_LOOKS } else { 1 };
        for _ in 0..looks {
            if !matches!(reader.read(LOOK_MAX), Ok(false)) {
                break;
            }
        }
    }
    Done { reader, located }
}

/// The usage that a transcript holds for the model message `message_id`: that of its line with a stop reason that has the
/// most output.
///
/// # Errors
/// When the file cannot be read.
pub fn closing_usage(transcript: &Path, message_id: &str) -> std::io::Result<Option<Json>> {
    let mut reader = Reader::at(transcript.to_path_buf());
    for _ in 0..FINAL_LOOKS {
        if reader.read(LOOK_MAX)? {
            break;
        }
    }
    Ok(reader
        .messages
        .get(message_id)
        .and_then(|seen| seen.usage.clone()))
}
