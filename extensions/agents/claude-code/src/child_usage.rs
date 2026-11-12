//! What a sub-agent's requests cost, settled from the sub-agent's own transcript.
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
//! The mapper only *hears* ([`Heard`]): a message printed, a sub-agent ended. The conversation hands that, and the
//! end of a turn, to a task of its own, the settler ([`Settling::spawn`]), which owns the held requests, looks at
//! the transcripts and gives its reports to the same sink the conversation uses. The conversation's loop never
//! waits for a file.
//!
//! A turn's completion goes out after the reports of what its sub-agents had made when it ended, so that the
//! engine, which closes a turn's usage when the completion arrives, never counts a request twice. The completion
//! waits in the conversation's outbox behind a gate, and **only the settler opens the gate** (or, when the settler
//! is gone, the conversation, after the settler has reported what it held): no clock of the conversation's lets a
//! completion overtake a report. That is safe because every wait of the settler is inside one budget, below.
//!
//! # Which turn counts a request
//!
//! A request is owned by the turn that started its sub-agent, and the engine counts it by that turn's row. A turn's
//! own use, as the program reports it, is the growth of its running totals since the result before: so a sub-agent
//! started in an earlier turn that makes a request between two results (the background-wake flow is exactly this:
//! the child's last request, then the wake's turn) is in the use of the turn whose result follows, and the engine
//! closes that turn as its use less the requests it owns. Left alone, the request is counted by its owner and again
//! in the shortfall of the later turn.
//!
//! The stretch between two results is a *window* of the totals (numbered once for the conversation, whatever
//! process or mapper serves it). A request belongs to the window whose growth holds its cost, which is not always the
//! window it is printed in: the program prints a message block by block, as each block is complete, and adds the cost
//! of the message to its totals when the message ends, so a result that comes between the first block and the end
//! (the usual start of a sub-agent is a message of a few words and a call) closes a window that does not hold it. The
//! mapper finds those by what the growth holds beyond the turn's own use ([`Heard::Placed`]); the settler gives each
//! request its window when the result is read. Each reported request is put in a ledger with its window, owner and
//! figures, and the turn that closes a window leaves out of its use ([`leave_out`]) the requests of the window that it
//! does not own, whatever their figures are (a request reported as the stream said it is left out as that, and the
//! rest of what it used stays in the later turn's shortfall: nothing is counted twice and nothing is lost).
//!
//! Two orders follow from how the engine closes a turn (its use, less the rows it has by then):
//! - the gate of a turn waits for what its window holds, so that the ledger is complete when the use goes out, and the
//!   use goes out behind the gate with the completion;
//! - a request that the use of its owner does not hold (the owner's window is closed and the gate does not wait for
//!   it) goes to the host *after* the owner's completion, by the conversation's outbox: the engine counts the rows
//!   that exist when a turn closes as part of its use, so a row sent before would hide the owner's own use.
//!
//! The thread's own total is the growth of the totals and is not changed. The engine's rule is not touched: it holds
//! for every adapter that reports a turn's use as its own. Where the settler is gone the ledger is written all the
//! same, as the conversation reports what is printed as the stream said it, so that the accounting stays conserved.
//!
//! # One budget
//!
//! The profile option `child_usage_wait_ms` (a second by default, `wait` below) is the budget. A request is
//! reported at the latest at its *deadline*: the moment it was last printed plus the quiet of a first look (100 ms)
//! plus `wait`, or, once its sub-agent or its turn has ended, the moment that happened plus `wait`, whichever is
//! later; once the program is gone, its files are complete and a request is reported after one last read. The
//! settler waits for nothing else:
//! - a read of a transcript runs on a blocking thread and answers by message; the settler's loop does not wait for
//!   it, and a request is reported at its deadline whatever the read is at. A read that has not answered within
//!   `wait` (a disk that has stopped answering) is left to itself and another is started; its answer, if it comes,
//!   is dropped;
//! - the exit of the program is waited for by a task of its own, at most [`EXIT_GRACE`], and the deadlines of the
//!   requests held at that time are moved by that much;
//! - the one wait left is the send of a report to the sink, which is the host's own back-pressure.
//!
//! # The look schedule
//!
//! Each held request has its own schedule: the first look 100 ms after its message was last printed (a transcript
//! shows a message about a tenth of a second after the program prints it), then a look after twice the last
//! interval, up to 5 s: looks at 0.1 s, 0.3 s, 0.7 s, 1.5 s, 3.1 s, 6.3 s, then every 5 s, never later than the
//! deadline. A request whose sub-agent has ended, or whose turn has ended, is *due*: it is looked at every 25 ms.
//! One request that never settles delays no other request's first look, and is reported at its deadline.
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
//! | The message names no sub-agent or session, a name is unusable, or the harness has no home | Reported at once as the stream said it |
//! | A request is not shown by its deadline (no file, no directory, a read that does not answer, a line never written) | Reported as the stream said it, with a debug line |
//! | The session shows no transcripts at all when a request of it reaches its deadline | That is remembered for the session: its other requests are reported at once, and so are later ones, as long as the first look still finds no directory |
//! | The sub-agent is still running when its turn ends | What it made so far is due at the turn's end, settled within the one budget; its later requests are looked for as they are written, and are due at its end |
//! | The program is lost | The requests held are read once more, to the end of the files, as soon as the program has exited (never later than [`EXIT_GRACE`]), and the rest reported as the stream said it |
//! | The settler fails | What it holds is reported as the stream said it, and the gates are opened; what is printed after is reported by the conversation as the stream said it, with its place in the ledger |
//! | Two sub-agents at once | Each request is settled from the transcript of its own agent, and a sub-agent's end makes due only its own requests |
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

/// How long after a message was printed its request is first looked for, and the first interval between looks.
pub const LOOK_FIRST: Duration = Duration::from_millis(100);
/// The longest interval between two looks for a request that is not due.
pub const LOOK_LAST: Duration = Duration::from_secs(5);
/// The interval between looks for a request that is due.
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
/// How many ended turns the settler remembers.
const ENDED_MAX: usize = 256;
/// How long a reader is kept after its last look when no request needs it.
const READER_IDLE: Duration = Duration::from_secs(60);
/// How many bytes at the start of a transcript are kept to tell that it was rewritten.
const HEAD_BYTES: usize = 64;

/// What the mapper heard of sub-agents, in the order it heard it.
#[derive(Debug, Clone)]
pub enum Heard {
    /// A message of a sub-agent was printed, in a turn: the request it describes waits for its transcript.
    Printed {
        /// The turn that started the sub-agent, which the request belongs to.
        turn: Id,
        /// The message.
        printed: Box<Printed>,
    },
    /// A result closed `window` of totals ([`crate::events::EventMapper::take_usage_window`]): every message printed
    /// so far is in it, and its cost is in the totals the window grew by, except the `unfinished` ones, which the
    /// program had not yet ended and whose cost is in a later window.
    Placed {
        /// The window the result closed.
        window: u64,
        /// The messages that are not in it, by the tool use that started their sub-agent and the model's message.
        unfinished: Vec<(String, String)>,
    },
    /// The sub-agent that the tool use started has ended.
    Ended {
        /// The tool use.
        parent: String,
    },
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
    Ended {
        parent: String,
    },
    Placed {
        window: u64,
        unfinished: Vec<Key>,
    },
    TurnEnded {
        turn: Id,
        window: Option<u64>,
        gate: Option<u64>,
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

/// What a reported request added to the totals of the window its cost entered, for the turn that closes the window to
/// leave out ([`Settling::others`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contribution {
    /// The request, by the tool use that started its sub-agent and the model's message; `None` for what was summed
    /// from several, which are placed together.
    pub key: Option<(String, String)>,
    /// The window of totals its cost is in; `None` while the window is not known: the message was reported before the
    /// result that closes its window, or it is unfinished and its window is a later one.
    pub window: Option<u64>,
    /// The turn that owns the request.
    pub owner: Id,
    /// The model, as the message named it.
    pub model: Option<String>,
    /// What the request used, as it was reported.
    pub tokens: TokenUsage,
}

/// How many contributions are kept one by one while their window is not known; older ones are summed by owner and
/// model (they cannot be unfinished, and are placed together).
const CONTRIBUTIONS_MAX: usize = 4096;

type Ledger = Arc<Mutex<Vec<Contribution>>>;

/// A request that goes to the host after the completion of the turn that owns it, by the conversation's outbox.
#[derive(Debug)]
pub struct Deferred {
    /// The segment the request was made in.
    pub segment: Id,
    /// The turn that owns it.
    pub turn: Id,
    /// The request.
    pub request: Box<UsageRequest>,
}

/// How the conversation talks to its settler.
#[derive(Debug)]
pub struct Settling {
    notes: mpsc::UnboundedSender<Note>,
    holds: Arc<AtomicUsize>,
    ledger: Ledger,
}

impl Settling {
    /// Starts the settler of a conversation. It reports to `sink` for `thread`; `home` is where the program keeps
    /// its transcripts; `wait` is the budget of the module. The numbers it sends back are the gates that are
    /// open: a gate is opened when everything it waited for has been reported. The requests that must go to the host
    /// after the completion of their turn come back, to be sent by the conversation's outbox.
    #[must_use]
    pub fn spawn(
        sink: EventSink,
        thread: Id,
        home: Option<PathBuf>,
        wait: Duration,
    ) -> (
        Self,
        mpsc::UnboundedReceiver<u64>,
        mpsc::UnboundedReceiver<Deferred>,
    ) {
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
    ) -> (
        Self,
        mpsc::UnboundedReceiver<u64>,
        mpsc::UnboundedReceiver<Deferred>,
    ) {
        let (notes, received) = mpsc::unbounded_channel();
        let (inner, inner_received) = mpsc::unbounded_channel();
        let (gates_sent, gates) = mpsc::unbounded_channel();
        let (deferred_sent, deferred) = mpsc::unbounded_channel();
        let holds = Arc::new(AtomicUsize::new(0));
        let ledger = Ledger::default();
        let settler = Settler {
            ledger: Arc::clone(&ledger),
            sink,
            thread,
            home,
            wait,
            files,
            holds: Arc::clone(&holds),
            gates: gates_sent,
            deferred: deferred_sent,
            inner,
            items: HashMap::new(),
            reported: VecDeque::new(),
            watches: HashMap::new(),
            none: HashSet::new(),
            waiters: Vec::new(),
            ended: VecDeque::new(),
            epochs: 0,
            closing: false,
        };
        tokio::spawn(supervise(settler, received, inner_received));
        (
            Self {
                notes,
                holds,
                ledger,
            },
            gates,
            deferred,
        )
    }

    /// Whether any request may still be held or on its way to the settler.
    #[must_use]
    pub fn holds(&self) -> bool {
        self.holds.load(Ordering::SeqCst) > 0
    }

    /// Whether the settler has failed or is over: nothing is held any longer, and what is printed is reported by the
    /// conversation as the stream said it.
    #[must_use]
    pub fn is_gone(&self) -> bool {
        self.notes.is_closed()
    }

    /// A message of a sub-agent was printed. Gives the request back when the settler is gone, to be reported as
    /// the stream said it; what it added to the totals is entered in the ledger all the same, so that the turn that
    /// closes its window leaves it out as it does when the settler reports.
    ///
    /// # Errors
    /// When the settler is gone.
    pub fn printed(
        &self,
        segment: Id,
        turn: Id,
        printed: Printed,
    ) -> Result<(), Box<UsageRequest>> {
        if self.notes.is_closed() {
            note_contribution(
                &self.ledger,
                Some((printed.parent.clone(), printed.message_id.clone())),
                None,
                turn,
                &printed.request,
            );
            return Err(Box::new(printed.request));
        }
        self.holds.fetch_add(1, Ordering::SeqCst);
        let _ = self.notes.send(Note::Printed(Box::new(Hold {
            segment,
            turn,
            printed,
        })));
        Ok(())
    }

    /// The sub-agent that the tool use `parent` started has ended.
    pub fn ended(&self, parent: String) {
        let _ = self.notes.send(Note::Ended { parent });
    }

    /// A result closed `window` of totals: every message printed so far is in it except the `unfinished` ones, whose
    /// cost the program adds to the totals when it ends them, in a later window.
    ///
    /// The ledger is placed here, at once: a turn that holds nothing has no gate, and its use goes out leaving out what
    /// the ledger holds for the window as soon as its result is read. The settler places what it holds, and what it
    /// reported in the meantime, when it takes the note up, which is before it takes up the end of the turn.
    pub fn placed(&self, window: u64, unfinished: Vec<(String, String)>) {
        place_in_ledger(&self.ledger, window, &unfinished);
        let _ = self.notes.send(Note::Placed { window, unfinished });
    }

    /// The turn has ended: what its sub-agents have made so far is due, and so is what is in the `window` its result
    /// closed, whoever owns it, since the turn's use is the growth of the totals over that window. The number of the
    /// `gate` comes back when all of it is reported; without a gate there is nothing to wait for. From now on the
    /// turn's requests that the gate does not wait for go to the host after the turn's completion.
    pub fn turn_ended(&self, turn: Id, window: Option<u64>, gate: Option<u64>) {
        let _ = self.notes.send(Note::TurnEnded { turn, window, gate });
    }

    /// What the requests reported for the `window`, which the turn `turn` does not own, added to the totals the
    /// turn's use was taken from: the turn's use is to leave it out, since the turn that owns the request counts it
    /// by the request. What was reported for this window and the ones before is forgotten.
    #[must_use]
    pub fn others(&self, window: u64, turn: Id) -> Vec<Contribution> {
        let mut ledger = self
            .ledger
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut others = Vec::new();
        ledger.retain(|contribution| match contribution.window {
            Some(known) if known == window && contribution.owner != turn => {
                others.push(contribution.clone());
                false
            }
            Some(known) => known > window,
            None => true,
        });
        others
    }

    /// Makes the settler fail, so that a case can see what is done when it does. Nothing else calls it.
    #[doc(hidden)]
    pub fn crash(&self) {
        let _ = self.notes.send(Note::Crash);
    }

    /// The program is gone, or is going once `exited` says so: every request is looked for a last time, once the
    /// program has exited, and reported.
    pub fn lost(&self, exited: Option<watch::Receiver<bool>>) {
        let _ = self.notes.send(Note::Lost { exited });
    }
}

/// Enters what a request added to the totals in the ledger of its window; `window` is where it is known.
fn note_contribution(
    ledger: &Ledger,
    key: Option<Key>,
    window: Option<u64>,
    owner: Id,
    request: &UsageRequest,
) {
    let mut ledger = ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    ledger.push(Contribution {
        key,
        window,
        owner,
        model: request.model.clone(),
        tokens: request.tokens.usage(),
    });
    // Those whose window is not known are kept one by one while there are not too many: only those can be
    // unfinished. The older ones are summed by owner and model, and placed together.
    let waiting = ledger.iter().filter(|each| each.window.is_none()).count();
    if waiting > CONTRIBUTIONS_MAX {
        let mut excess = waiting - CONTRIBUTIONS_MAX / 2;
        let mut sums: Vec<Contribution> = Vec::new();
        ledger.retain(|each| {
            if each.window.is_some() || excess == 0 {
                return true;
            }
            excess -= 1;
            match sums
                .iter_mut()
                .find(|sum| sum.owner == each.owner && sum.model == each.model)
            {
                Some(sum) => sum.tokens = sum.tokens.plus(each.tokens),
                None => sums.push(Contribution {
                    key: None,
                    ..each.clone()
                }),
            }
            false
        });
        ledger.extend(sums);
    }
}

/// A result closed `window`: the contributions whose window was not known are in it, except the unfinished ones; those
/// of one window are summed by owner and model.
fn place_in_ledger(ledger: &Ledger, window: u64, unfinished: &[Key]) {
    let mut ledger = ledger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut placed: Vec<Contribution> = Vec::new();
    ledger.retain_mut(|each| {
        if each.window.is_some() {
            return true;
        }
        if each
            .key
            .as_ref()
            .is_some_and(|key| unfinished.contains(key))
        {
            return true;
        }
        match placed
            .iter_mut()
            .find(|sum| sum.owner == each.owner && sum.model == each.model)
        {
            Some(sum) => sum.tokens = sum.tokens.plus(each.tokens),
            None => placed.push(Contribution {
                key: None,
                window: Some(window),
                ..each.clone()
            }),
        }
        false
    });
    ledger.extend(placed);
}

/// A request is known by the tool use that started its sub-agent and by the model's message.
type Key = (String, String);
/// A sub-agent is known by its session and its name.
type Group = (String, String);

#[derive(Debug)]
struct Item {
    request: UsageRequest,
    turn: Id,
    /// The window of totals its cost is in, once a result has closed it; `None` for a message the program has not
    /// ended when a result comes, and for one printed after the last result.
    window: Option<u64>,
    segment: Id,
    agent: String,
    session: String,
    /// Its sub-agent or its turn has ended: the gate of the turn waits for it.
    due: bool,
    /// When it is reported as the stream said it if the transcript does not show it.
    deadline: Instant,
    next_look: Instant,
    every: Duration,
    /// Its session was found to have no transcripts: it is reported at once while the first look finds none.
    quick: bool,
    /// The program is stopping (the number of that stop): the last read waits for its exit.
    exit: Option<u64>,
    /// The program has exited: the first read that begins after this is the last.
    last_since: Option<Instant>,
}

impl Item {
    fn make_due(&mut self, now: Instant, wait: Duration) {
        self.due = true;
        self.deadline = self.deadline.max(now + wait);
        self.every = LOOK_DUE;
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
    turn: Id,
    window: Option<u64>,
    /// The requests the gate waits for: those held when the turn ended. A request that a sub-agent that goes on makes
    /// later is not waited for, even though the turn owns it or another turn's end made it due.
    keys: Vec<Key>,
}

impl Waiter {
    /// Whether a request is one that the gate waits for: one whose cost is in the window the turn's result closed (or
    /// an earlier one), whoever owns it, since the turn's use is the growth of the totals over the window. A message
    /// the program had not ended is not: its cost is in a later window, and the turn's use does not hold it. Where
    /// the turn has no window (the totals it grew from are not known) it is the turn's own requests.
    fn waits_for(&self, item: &Item) -> bool {
        match (self.window, item.window) {
            (Some(window), Some(of_item)) => of_item <= window,
            (Some(_), None) => false,
            (None, _) => item.turn == self.turn,
        }
    }
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
    holds: Arc<AtomicUsize>,
    gates: mpsc::UnboundedSender<u64>,
    deferred: mpsc::UnboundedSender<Deferred>,
    inner: mpsc::UnboundedSender<Inner>,
    items: HashMap<Key, State>,
    reported: VecDeque<Key>,
    watches: HashMap<Group, Watch>,
    none: HashSet<String>,
    waiters: Vec<Waiter>,
    /// The turns that have ended, the latest last: what a turn owns that its gate does not wait for goes to the host
    /// after the turn's completion.
    ended: VecDeque<Id>,
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
                    Some(note) => self.hear(note).await,
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
        self.wait.max(LOOK_FIRST)
    }

    fn next_wake(&self) -> Option<Instant> {
        let stuck = self.stuck();
        self.items
            .values()
            .filter_map(|state| match state {
                State::Held(item) => {
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
                    Some(item.deadline.min(look))
                }
                State::Reported => None,
            })
            .min()
    }

    async fn hear(&mut self, note: Note) {
        let now = Instant::now();
        match note {
            Note::Printed(hold) => {
                let Hold {
                    segment,
                    turn,
                    printed,
                } = *hold;
                self.printed(segment, turn, printed).await;
            }
            Note::Placed { window, unfinished } => {
                for (key, state) in &mut self.items {
                    if let State::Held(item) = state
                        && item.window.is_none()
                        && !unfinished.contains(key)
                    {
                        item.window = Some(window);
                    }
                }
                place_in_ledger(&self.ledger, window, &unfinished);
            }
            Note::Crash => panic!("the settler was made to fail"),
            Note::Ended { parent } => {
                let wait = self.wait;
                for ((owner, _), state) in &mut self.items {
                    if *owner == parent
                        && let State::Held(item) = state
                    {
                        item.make_due(now, wait);
                    }
                }
            }
            Note::TurnEnded { turn, window, gate } => {
                let wait = self.wait;
                if !self.ended.contains(&turn) {
                    self.ended.push_back(turn);
                    while self.ended.len() > ENDED_MAX {
                        self.ended.pop_front();
                    }
                }
                if let Some(gate) = gate {
                    let mut waiter = Waiter {
                        gate,
                        turn,
                        window,
                        keys: Vec::new(),
                    };
                    for (key, state) in &mut self.items {
                        if let State::Held(item) = state
                            && waiter.waits_for(item)
                        {
                            item.make_due(now, wait);
                            waiter.keys.push(key.clone());
                        }
                    }
                    self.waiters.push(waiter);
                }
            }
            Note::Lost { exited } => self.lost(exited, now),
        }
    }

    async fn printed(&mut self, segment: Id, turn: Id, printed: Printed) {
        let Printed {
            request,
            parent,
            agent_id,
            session_id,
            message_id,
        } = printed;
        let key = (parent, message_id);
        let now = Instant::now();
        let wait = self.wait;
        match self.items.get_mut(&key) {
            Some(State::Held(item)) => {
                // Another block of a message that waits: it names more tool calls, and the message is not looked
                // for before the quiet of a first look has passed.
                merge_refs(&mut item.request, &request);
                if !item.due {
                    item.next_look = item.next_look.max(now + LOOK_FIRST);
                    item.deadline = item.deadline.max(now + LOOK_FIRST + wait);
                }
                self.holds.fetch_sub(1, Ordering::SeqCst);
                return;
            }
            Some(State::Reported) => {
                tracing::debug!(
                    request = %request.id,
                    "a sub-agent's message was printed again after its request was reported"
                );
                self.holds.fetch_sub(1, Ordering::SeqCst);
                return;
            }
            None => {}
        }
        let never = match (&self.home, &agent_id, &session_id) {
            (None, ..) => Some("the harness has no home to read"),
            (_, None, _) | (_, _, None) => Some("the message names no sub-agent or no session"),
            (_, Some(agent), Some(session))
                if !usable(agent) || !usable(session) || key.1.is_empty() =>
            {
                Some("the names of the request are not usable")
            }
            _ => None,
        };
        let (Some(agent), Some(session), None) = (agent_id, session_id, never) else {
            self.mark_reported(key.clone());
            self.send(segment, turn, &key, None, request, None, never)
                .await;
            return;
        };
        let quick = self.none.contains(&session);
        self.items.insert(
            key,
            State::Held(Box::new(Item {
                request,
                turn,
                window: None,
                segment,
                agent,
                session,
                due: false,
                deadline: now + LOOK_FIRST + wait,
                next_look: if quick { now } else { now + LOOK_FIRST },
                every: LOOK_FIRST,
                quick,
                exit: None,
                last_since: None,
            })),
        );
    }

    /// The program is gone, or is going once it has exited: the requests held now are read one last time then.
    fn lost(&mut self, exited: Option<watch::Receiver<bool>>, now: Instant) {
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
                item.deadline = item.deadline.max(now + EXIT_GRACE + wait);
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
                && (epoch.is_none() || item.exit == epoch)
            {
                item.exit = None;
                item.last_since = Some(now);
                item.next_look = now;
                item.deadline = now + wait;
            }
        }
    }

    /// The conversation is over: what is held is read to the end and reported.
    fn close(&mut self) {
        self.closing = true;
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
                State::Held(item) if item.deadline <= now => Some(key.clone()),
                _ => None,
            })
            .collect();
        for key in late {
            let Some(State::Held(item)) = self.items.get(&key) else {
                continue;
            };
            let session = item.session.clone();
            let group = (item.session.clone(), item.agent.clone());
            let (located, reading) = self
                .watches
                .get(&group)
                .map_or((Located::NoFile, false), |watch| {
                    (watch.located.clone(), watch.reading.is_some())
                });
            let why = match &located {
                Located::NoDirectory => "the session shows no transcripts",
                _ if reading => "the transcript could not be read in time",
                Located::NoFile => "the transcript of the sub-agent is not there",
                Located::File(_) => "the transcript does not show the request finished",
            };
            self.report(&key, None, Some(why)).await;
            if located == Located::NoDirectory {
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
            if let State::Held(item) = state {
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
                State::Held(item) if item.session == group.0 && item.agent == group.1 => {
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
        item.every = if item.due {
            LOOK_DUE
        } else {
            (item.every * 2).min(LOOK_LAST)
        };
        item.next_look = (now + item.every).min(item.deadline);
    }

    /// The session has no transcripts: what it still holds is reported at once, and nothing waits for it.
    async fn no_transcripts(&mut self, session: String) {
        let keys: Vec<Key> = self
            .items
            .iter()
            .filter_map(|(key, state)| match state {
                State::Held(item) if item.session == session => Some(key.clone()),
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
        let Item {
            mut request,
            turn,
            window,
            segment,
            ..
        } = *item;
        for call in &seen.refs {
            let known = request.tool_refs.get_or_insert_with(Vec::new);
            if !known.contains(call) {
                known.push(call.clone());
            }
        }
        self.send(segment, turn, key, window, request, seen.usage.as_ref(), None)
            .await;
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
        let Item {
            request,
            turn,
            window,
            segment,
            ..
        } = *item;
        self.send(segment, turn, key, window, request, usage, why)
            .await;
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the facts of one report: where it goes, whose it is, what it is and how it was settled"
    )]
    async fn send(
        &mut self,
        segment: Id,
        turn: Id,
        key: &Key,
        window: Option<u64>,
        mut request: UsageRequest,
        usage: Option<&Json>,
        why: Option<&'static str>,
    ) {
        if let Some(usage) = usage {
            settle_usage(&mut request, usage);
        } else if let Some(why) = why {
            tracing::debug!(
                request = %request.id,
                why,
                "a sub-agent's request is reported as its stream said it"
            );
        }
        // What the request used is in the totals of its window, and the turn that closes the window leaves it out of
        // its use, whatever the figure is: the turn that owns the request counts it by the request.
        note_contribution(&self.ledger, Some(key.clone()), window, turn, &request);
        // The engine closes a turn as its use less the requests it owns that it has by then: a request the turn's use
        // does not hold (its cost came after the result) is to be counted after the close, not within it. So the
        // request of a turn that has ended goes to the host after the turn's completion, by the outbox, unless a gate
        // waits for it (its cost is in the use of a turn whose gate is still shut).
        if self.ended.contains(&turn)
            && !self
                .waiters
                .iter()
                .any(|waiter| waiter.keys.contains(key))
        {
            // A conversation that is over takes nothing back: the request goes to the host from here.
            if let Err(mpsc::error::SendError(handed)) = self.deferred.send(Deferred {
                segment,
                turn,
                request: Box::new(request),
            }) {
                let _ = self
                    .sink
                    .request(self.thread, handed.segment, handed.turn, *handed.request)
                    .await;
            }
        } else {
            let _ = self.sink.request(self.thread, segment, turn, request).await;
        }
        self.holds.fetch_sub(1, Ordering::SeqCst);
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
            // Only what was held when the turn ended is waited for: a request written later by a sub-agent that goes on
            // is not.
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
                State::Held(item) => Some((item.session.clone(), item.agent.clone())),
                State::Reported => None,
            })
            .collect();
        self.watches.retain(|group, watch| {
            held.contains(group) || watch.reading.is_some() || watch.used + READER_IDLE > now
        });
    }

    /// The settler failed: what it holds is reported as the streams said it, and nothing waits for it.
    async fn release_all(&mut self) {
        let keys: Vec<Key> = self
            .items
            .iter()
            .filter(|(_, state)| matches!(state, State::Held(_)))
            .map(|(key, _)| key.clone())
            .collect();
        for key in keys {
            self.report(&key, None, Some("the settler failed")).await;
        }
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
/// sub-agent started in an earlier turn used while this one ran (or between the two). The engine closes a turn as
/// its use less the requests the turn owns, and the request of the earlier turn is counted by that turn: left in, it
/// would be counted twice. The thread's own total is the growth of the totals as it is, and stays so.
///
/// Each part is taken out by itself (the input, its cache parts, the output, the reasoning). A row that carries no
/// reasoning leaves none out, and the reasoning of a turn is a part of its output, so what the output that is left
/// cannot hold is dropped: that reasoning is the request's own, which no row could carry (the transcript does not
/// count it apart), and the total, input and output, is conserved whatever the parts.
pub fn leave_out(usage: &mut UsageUpdated, others: &[Contribution]) {
    for other in others {
        usage.turn = crate::events::less(usage.turn, other.tokens);
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
            entry.tokens = crate::events::less(entry.tokens, other.tokens);
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
