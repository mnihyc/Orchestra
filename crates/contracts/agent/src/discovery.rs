//! Sessions a harness already has; the models an endpoint lists, each with the name that is sent, the name that is shown and its rate where the list carries one; and the MCP servers of its own configuration, reported by name, with one definition read out when it is imported.
//!
//! Three things an adapter or a wire protocol finds out rather than being told.
//!
//! **Sessions** the harness already holds ([`NativeSession`]), so that a person can adopt one as a thread, and
//! what the harness recorded of one ([`SessionRecord`]): the model it ran with, and its earlier turns as entries
//! ([`RecordedEntry`]), what a person said and what the agent did, which become the thread's items from the
//! session before Orch.
//!
//! **Models**. A list of models, a harness's own or an endpoint's, is read for both names a model has: the
//! routing name, which is sent upstream exactly as it is and is opaque to Orch, and the display name that
//! people see ([`ModelSuggestion`]). A gateway's list may also carry prices; [`pricing_to_rate`] turns
//! them into the rate that is proposed for the model. A suggestion is a proposal: a model is usable once a
//! manager accepted it. Wherever models are named by a pattern (a rate card, a cap, an allowlist) one rule
//! holds, which is [`ModelPattern`].
//!
//! **MCP servers** that the harness has configured on a machine. They are *found* by name
//! ([`McpFound`]: never a command, an address or a secret), and one is *imported* by reading its definition
//! out ([`McpDefinition`]), which the connector keeps on that machine and nowhere else.

use std::collections::BTreeMap;

use orch_plugin_contract::Secret;
use orch_types::{Hash, Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::parameters::Rate;

/// A session that a harness already has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeSession {
    /// The harness's id for it.
    pub native_session_ref: String,
    /// Its title, where the harness keeps one.
    pub title: Option<String>,
    /// The directory it worked in.
    pub cwd: Option<String>,
    /// When it was last used.
    pub updated_at: Option<Time>,
    /// The thread that has adopted it, when one has; filled in by the API and never by an adapter.
    pub adopted_thread_id: Option<Id>,
}

/// The format of what an adapter writes of a session's record for its adoption: one [`RecordedEntry`] to a
/// line, oldest first.
pub const RECORD_FORMAT: &str = "orch-record-jsonl-v1";

/// The most a written record may hold: 32 MiB. A session that recorded more keeps its latest turns that fit,
/// whole, and the report says how many earlier ones were left out; the harness keeps them all the same.
pub const RECORD_MAX_BYTES: usize = 32 * 1024 * 1024;

/// What a harness recorded of a session it holds, as an adapter reads it when a person adopts the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionRecord {
    /// The model the session ran with last, where the harness says.
    pub model: Option<String>,
    /// The reasoning effort it ran with last, where the harness says.
    pub effort: Option<String>,
    /// How many of its turns the record holds.
    pub turns: i64,
    /// How many entries they come to.
    pub entries: i64,
    /// The file of entries ([`RECORD_FORMAT`]) the adapter wrote, for the connector to upload; `None` when it
    /// was not asked to write one, or the session holds nothing.
    pub file: Option<crate::history::SessionArchive>,
    /// What became of what the harness recorded: kept by kind, and what was left out and why.
    pub report: crate::history::HistoryReport,
}

/// One line of a session's record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "entry", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "an entry is read or written once, a line at a time; boxing the item would put a layer in every use"
)]
pub enum RecordedEntry {
    /// What a person said: the words that began a turn, or that were added to it while it ran.
    Said(RecordedWords),
    /// What the agent said or did, as the adapter maps it for a live session.
    Item(RecordedItem),
}

impl RecordedEntry {
    /// The turn it belongs to, counted from the first of the session (1, 2, 3 …).
    #[must_use]
    pub const fn turn(&self) -> i64 {
        match self {
            Self::Said(words) => words.turn,
            Self::Item(item) => item.turn,
        }
    }
}

/// A person's words, as the harness recorded them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecordedWords {
    /// A name for them that is unique within the session: `said:` and the harness's own id.
    pub native_ref: String,
    /// The turn, counted from the first.
    pub turn: i64,
    /// What was said.
    pub text: String,
    /// How many pictures and files came with the words, which are not carried.
    pub left_out: i64,
    /// When, where the harness says.
    pub at: Option<Time>,
}

/// What the agent said or did, as the harness recorded it: the item the adapter would report for it live, with
/// the same `native_ref`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RecordedItem {
    /// The adapter's name for it, unique within the session.
    pub native_ref: String,
    /// The turn, counted from the first.
    pub turn: i64,
    /// `main`, or `sub:<id>` for the items of a sub-agent.
    pub stream: String,
    /// For an item of a sub-agent, the item that started it.
    pub parent_native_ref: Option<String>,
    /// How it ended.
    pub status: crate::events::ItemStatus,
    /// Its kind and its last payload.
    pub content: crate::events::ItemContent,
    /// When, where the harness says.
    pub at: Option<Time>,
}

/// A place in a turn of a record being built: an entry that is whole, or an item that began and has not ended.
#[derive(Debug, Clone)]
enum Slot {
    Whole(Box<RecordedEntry>),
    Begun(Box<crate::events::ItemStarted>, Option<Time>),
}

/// Builds the turns of a record from what a person said and the events an adapter's mapping makes of the rest
/// of what the harness recorded: each item stands where it began, with the payload it ended with. An item that
/// never ended in the record is written as cancelled.
#[derive(Debug, Clone, Default)]
pub struct RecordBuilder {
    turns: Vec<Vec<Slot>>,
    begun: BTreeMap<String, (usize, usize)>,
}

impl RecordBuilder {
    /// An empty record.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of the turn being built, counted from 1; 0 before the first.
    #[must_use]
    pub fn turn(&self) -> i64 {
        i64::try_from(self.turns.len()).unwrap_or(i64::MAX)
    }

    /// A new turn begins.
    pub fn begin_turn(&mut self) {
        self.turns.push(Vec::new());
    }

    /// A person said `text` (with `left_out` pictures or files that are not carried), in the turn being built,
    /// under `native_id`, the harness's id for the words.
    pub fn said(&mut self, native_id: &str, text: String, left_out: i64, at: Option<Time>) {
        if self.turns.is_empty() {
            self.begin_turn();
        }
        let turn = self.turn();
        if let Some(slots) = self.turns.last_mut() {
            slots.push(Slot::Whole(Box::new(RecordedEntry::Said(RecordedWords {
                native_ref: format!("said:{native_id}"),
                turn,
                text,
                left_out,
                at,
            }))));
        }
    }

    /// What the mapping made of a step of the record, which happened at `at`: items that began are placed at the
    /// end of the turn being built, and items that ended take the payload and the status they ended with.
    pub fn events(&mut self, events: Vec<crate::events::SessionEvent>, at: Option<Time>) {
        for event in events {
            match event {
                crate::events::SessionEvent::ItemStarted(started) => {
                    if self.turns.is_empty() {
                        self.begin_turn();
                    }
                    let turn = self.turns.len() - 1;
                    let slots = &mut self.turns[turn];
                    self.begun
                        .insert(started.native_ref.clone(), (turn, slots.len()));
                    slots.push(Slot::Begun(Box::new(started), at));
                }
                crate::events::SessionEvent::ItemCompleted(completed) => {
                    let Some((turn, index)) = self.begun.remove(&completed.native_ref) else {
                        continue;
                    };
                    let Some(Slot::Begun(started, began)) = self.turns[turn].get(index).cloned()
                    else {
                        continue;
                    };
                    let started = *started;
                    let content = serde_json::from_value::<crate::events::ItemContent>(
                        serde_json::json!({ "kind": started.content.kind(), "payload": completed.payload }),
                    )
                    .unwrap_or(started.content);
                    self.turns[turn][index] =
                        Slot::Whole(Box::new(RecordedEntry::Item(RecordedItem {
                            native_ref: completed.native_ref,
                            turn: i64::try_from(turn + 1).unwrap_or(i64::MAX),
                            stream: started.stream,
                            parent_native_ref: started.parent_native_ref,
                            status: completed.status,
                            content,
                            at: began.or(at),
                        })));
                }
                _ => {}
            }
        }
    }

    /// The turns of the record, each with its entries in the order they began.
    #[must_use]
    pub fn finish(self) -> Vec<Vec<RecordedEntry>> {
        self.turns
            .into_iter()
            .enumerate()
            .map(|(index, slots)| {
                let turn = i64::try_from(index + 1).unwrap_or(i64::MAX);
                slots
                    .into_iter()
                    .map(|slot| match slot {
                        Slot::Whole(entry) => *entry,
                        Slot::Begun(started, at) => RecordedEntry::Item(RecordedItem {
                            native_ref: started.native_ref,
                            turn,
                            stream: started.stream,
                            parent_native_ref: started.parent_native_ref,
                            status: crate::events::ItemStatus::Cancelled,
                            content: started.content,
                            at,
                        }),
                    })
                    .collect()
            })
            .collect()
    }
}

/// The lines of a record of `turns` (each the entries of one turn, oldest first), bounded by `max_bytes`: the
/// latest turns that fit, whole. Answers the text, how many turns and entries it holds, and how many earlier
/// turns were left out to fit.
///
/// # Errors
/// An entry that cannot be written as JSON.
pub fn record_lines(
    turns: &[Vec<RecordedEntry>],
    max_bytes: usize,
) -> Result<(String, i64, i64, i64), serde_json::Error> {
    let mut written: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut kept_turns = 0i64;
    let mut entries = 0i64;
    for turn in turns.iter().rev() {
        let mut lines = String::new();
        for entry in turn {
            lines.push_str(&serde_json::to_string(entry)?);
            lines.push('\n');
        }
        if total + lines.len() > max_bytes {
            break;
        }
        total += lines.len();
        kept_turns += 1;
        entries += i64::try_from(turn.len()).unwrap_or(i64::MAX);
        written.push(lines);
    }
    written.reverse();
    let left_out = i64::try_from(turns.len()).unwrap_or(i64::MAX) - kept_turns;
    Ok((written.concat(), kept_turns, entries, left_out))
}

/// What became of what a session recorded, kind by kind, for the `turns` a record holds (each the entries of one
/// turn): every entry is kept, a picture or a file given with a person's words is dropped, and a warning says
/// how many earlier turns were left out and that the sub-agents' own conversations stay in the harness.
#[must_use]
pub fn record_report(
    turns: &[Vec<RecordedEntry>],
    left_out_turns: i64,
) -> crate::history::HistoryReport {
    let mut report = RecordReporter::default();
    for entry in turns.iter().flatten() {
        report.push(entry);
    }
    report.finish(left_out_turns)
}

/// Counts a structured record one entry at a time without retaining its turns.
#[derive(Debug, Default)]
pub struct RecordReporter {
    report: crate::history::ReportBuilder,
    subagents: bool,
}

impl RecordReporter {
    /// Adds one entry and its already counted attachments.
    pub fn push(&mut self, entry: &RecordedEntry) {
        match entry {
            RecordedEntry::Said(words) => {
                self.report.keep("user_message");
                self.report.drop_many(
                    "attachment",
                    "a picture or a file a person gave the harness is not carried",
                    words.left_out,
                );
            }
            RecordedEntry::Item(item) => {
                let kind = serde_json::to_value(item.content.kind())
                    .ok()
                    .and_then(|kind| kind.as_str().map(str::to_owned))
                    .unwrap_or_default();
                self.subagents |= matches!(&item.content, crate::events::ItemContent::ToolCall(call)
                    if matches!(call.tool, crate::events::ToolPayload::Subagent { .. }));
                self.report.keep(&kind);
            }
        }
    }

    /// Finishes the same report as a collected record, including omitted-turn warnings.
    #[must_use]
    pub fn finish(mut self, left_out_turns: i64) -> crate::history::HistoryReport {
        if left_out_turns > 0 {
            self.report.warn(&format!("{left_out_turns} earlier turns are not on the thread: what is taken in is bounded, and the harness keeps them"));
        }
        if self.subagents {
            self.report
                .warn("what its sub-agents said among themselves stays in the harness");
        }
        self.report.finish(crate::history::Fidelity::Structured)
    }
}

/// Reads a record written by [`record_lines`]: one entry for each line that is one. Lines that are not are
/// skipped and counted.
#[must_use]
pub fn read_record(text: &str) -> (Vec<RecordedEntry>, i64) {
    let mut entries = Vec::new();
    let mut unreadable = 0i64;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<RecordedEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(_) => unreadable += 1,
        }
    }
    (entries, unreadable)
}

/// A model as a list of models gives it, written once with the provider's contract.
pub use orch_provider_contract::models::ModelSuggestion;

/// A pattern that names models by their routing names.
///
/// The pattern is compared with a model's routing name, both in lower case. `*` stands for any run of
/// characters, and a pattern without one names one model. A pattern that holds no `/` is compared with what
/// follows the last `/` of the name, so that `gpt-6*` covers `gpt-6-sol` and `openai/gpt-6-sol` alike: the
/// same model through a gateway is the same family. A pattern that holds a `/` is compared with the whole
/// name, for whoever means one route (`openai/*`). Where one pattern of a list has to be chosen (a rate),
/// the one with the most characters that are not `*` counts ([`ModelPattern::best`]).
///
/// A list that says which models something covers (a cap, an allowlist, an entry's models) may also name
/// a **group**: `@frontier` names every model whose entry at the provider concerned is in the group
/// `frontier` that the provider's managers made ([`ModelPattern::list_names`]). The rate card and the
/// catalogue take no group: there a pattern selects a price by a name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelPattern {
    pattern: String,
}

/// The mark that begins a pattern which names a group of models and not a model.
pub const GROUP_MARK: char = '@';

impl ModelPattern {
    /// A pattern, read in lower case.
    #[must_use]
    pub fn new(pattern: &str) -> Self {
        Self {
            pattern: pattern.to_lowercase(),
        }
    }

    /// The group a pattern names, if it names one (`@frontier` names `frontier`).
    #[must_use]
    pub fn group_of(pattern: &str) -> Option<&str> {
        pattern.strip_prefix(GROUP_MARK)
    }

    /// Whether any pattern of `patterns`, a list that says which models something covers, names the model
    /// with routing name `model_name`, whose entry at the provider concerned is in `groups`: by its name,
    /// by a family, or by a group it is in. A group no model is in names nothing.
    #[must_use]
    pub fn list_names(patterns: &[String], model_name: &str, groups: &[String]) -> bool {
        patterns
            .iter()
            .any(|pattern| match Self::group_of(pattern) {
                Some(group) => groups
                    .iter()
                    .any(|member| member.eq_ignore_ascii_case(group)),
                None => Self::new(pattern).matches(model_name),
            })
    }

    /// How many of its characters are not `*`: how particular it is.
    #[must_use]
    pub fn literal_len(&self) -> usize {
        self.pattern.chars().filter(|c| *c != '*').count()
    }

    /// Whether it names the model with routing name `model_name`.
    #[must_use]
    pub fn matches(&self, model_name: &str) -> bool {
        let name = model_name.to_lowercase();
        let subject = if self.pattern.contains('/') {
            name.as_str()
        } else {
            name.rsplit('/').next().unwrap_or_default()
        };
        wildcard_matches(&self.pattern, subject)
    }

    /// Of `candidates`, each a pattern and what it stands for, the one that names `model_name` with the
    /// most characters that are not `*`. Of two that are equally particular the one that comes first
    /// counts. `None` when no pattern names the model.
    #[must_use]
    pub fn best<'a, T>(
        candidates: impl IntoIterator<Item = (&'a str, T)>,
        model_name: &str,
    ) -> Option<T> {
        let mut best: Option<(usize, T)> = None;
        for (pattern, value) in candidates {
            let pattern = Self::new(pattern);
            if !pattern.matches(model_name) {
                continue;
            }
            let particular = pattern.literal_len();
            if best.as_ref().is_none_or(|(most, _)| particular > *most) {
                best = Some((particular, value));
            }
        }
        best.map(|(_, value)| value)
    }
}

/// Whether `text` is matched by `pattern`, in which `*` is any run of characters (also none) and every
/// other character stands for itself.
fn wildcard_matches(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut resume: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            resume = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star, matched)) = resume {
            p = star + 1;
            t = matched + 1;
            resume = Some((star, matched + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// The prices a gateway's list gives for a model: US dollars for one token, as decimal strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GatewayPricing {
    /// A token of input.
    pub prompt: String,
    /// A token of output.
    pub completion: String,
    /// A cached token of input, where the list says.
    #[serde(default)]
    pub input_cache_read: Option<String>,
    /// A token written to a prompt cache, where the list says.
    #[serde(default)]
    pub input_cache_write: Option<String>,
}

/// A decimal number as an integer and the number of digits after the point; exact where `f64` is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Decimal {
    mantissa: u128,
    scale: u32,
}

impl Decimal {
    /// Reads a plain decimal such as `0.0000025`: digits, an optional point and more digits, no sign, no
    /// exponent.
    fn parse(text: &str) -> Option<Self> {
        let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
        if whole.is_empty() && fraction.is_empty() {
            return None;
        }
        if !whole
            .chars()
            .chain(fraction.chars())
            .all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let digits = format!("{whole}{fraction}");
        Some(Self {
            mantissa: digits.parse().ok()?,
            scale: u32::try_from(fraction.len()).ok()?,
        })
    }

    fn times(self, other: Self) -> Option<Self> {
        Some(Self {
            mantissa: self.mantissa.checked_mul(other.mantissa)?,
            scale: self.scale.checked_add(other.scale)?,
        })
    }

    /// This number times a million, as a decimal string without trailing zeros.
    fn per_million(self) -> Option<String> {
        const MILLION_DIGITS: u32 = 6;
        let (mantissa, scale) = if self.scale >= MILLION_DIGITS {
            (self.mantissa, self.scale - MILLION_DIGITS)
        } else {
            (
                self.mantissa
                    .checked_mul(10u128.checked_pow(MILLION_DIGITS - self.scale)?)?,
                0,
            )
        };
        let digits = mantissa.to_string();
        let scale = usize::try_from(scale).ok()?;
        let padded = format!("{digits:0>width$}", width = scale + 1);
        let (whole, fraction) = padded.split_at(padded.len() - scale);
        let fraction = fraction.trim_end_matches('0');
        Some(if fraction.is_empty() {
            whole.to_owned()
        } else {
            format!("{whole}.{fraction}")
        })
    }
}

/// The price of a million tokens, in credits, of a price for one token in dollars: both decimal strings,
/// `scale` being how many credits a dollar is worth in the provider's home (`usage.catalogue_scale`). The
/// arithmetic is exact. `None` when either is not a plain decimal.
#[must_use]
pub fn per_token_to_per_mtok(dollars_per_token: &str, scale: &str) -> Option<String> {
    Decimal::parse(dollars_per_token)?
        .times(Decimal::parse(scale)?)?
        .per_million()
}

/// The rate a gateway's prices propose for a model, in credits for a million tokens. A price the list does
/// not carry for cached input is the input price; a price for writing to a cache is left out when the list
/// has none, which counts it as input. `None` when a price is not a plain decimal.
#[must_use]
pub fn pricing_to_rate(pricing: &GatewayPricing, scale: &str) -> Option<Rate> {
    let input = per_token_to_per_mtok(&pricing.prompt, scale)?;
    let cached = match &pricing.input_cache_read {
        Some(price) => per_token_to_per_mtok(price, scale)?,
        None => input.clone(),
    };
    let cache_write = match &pricing.input_cache_write {
        Some(price) => Some(per_token_to_per_mtok(price, scale)?),
        None => None,
    };
    Some(Rate {
        input_per_mtok: input,
        cached_input_per_mtok: cached,
        output_per_mtok: per_token_to_per_mtok(&pricing.completion, scale)?,
        cache_write_per_mtok: cache_write,
        tiers: None,
        above: None,
    })
}

/// Which transport an MCP server speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// A program the MCP host starts inside the environment.
    Stdio,
    /// An address reached from inside the environment.
    Http,
}

/// Where a found MCP server was configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpScope {
    /// In its user's own configuration.
    User,
    /// In a directory of the workspace.
    Directory,
}

/// An MCP server that a harness has configured on a machine, reported by name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct McpFound {
    /// Identity of the transport, launch/address and variable names, excluding all variable values and secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_fingerprint: Option<Hash>,
    /// Its name in that configuration.
    pub name: String,
    /// The harness kind whose configuration names it.
    pub source: String,
    /// Where it is configured.
    pub scope: McpScope,
    /// For a server of a directory, that directory.
    pub directory: Option<String>,
    /// How it is reached.
    pub transport: McpTransport,
    /// A digest that changes when its definition does.
    pub digest: String,
}

/// Where to look for MCP servers beside the user's own configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpSearch {
    /// Directories of the workspace whose own configuration is read as well.
    #[serde(default)]
    pub directories: Vec<String>,
}

/// Which found server to read the definition of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpImportRequest {
    /// The id Orch gave the server; the connector keeps the copy under it.
    pub server_id: Id,
    /// The harness kind whose configuration names it.
    pub source: String,
    /// Where it is configured.
    pub scope: McpScope,
    /// Its name there.
    pub name: String,
    /// For a server of a directory, that directory.
    pub directory: Option<String>,
}

/// What the answer to an import says of the server whose definition was kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpImported {
    /// Structural identity used to recognize the same imported definition on another machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition_fingerprint: Option<Hash>,
    /// How it is reached.
    pub transport: McpTransport,
    /// A digest that changes when its definition does.
    pub digest: String,
    /// How many tools it listed, where that was learned.
    pub tools: Option<i64>,
}

/// Where a started MCP server's working directory is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpWorkingDir {
    /// The environment's workspace.
    #[default]
    Workspace,
    /// The user's home.
    Home,
}

/// What has to be there before a server can run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpRequires {
    /// Commands that have to be present in the environment.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Capabilities that have to be granted.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// The definition of an MCP server: how to start or reach it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpServerSpec {
    /// How it is reached.
    pub transport: McpTransport,
    /// For stdio: the program and its arguments, run inside the environment.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// For http: the address, reached from inside the environment.
    #[serde(default)]
    pub url: Option<String>,
    /// Authentication for an HTTP server. Only a secret's name is public; its value travels as a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authentication: Option<McpAuthentication>,
    /// Plain variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Names of variables whose values are secrets.
    #[serde(default)]
    pub secret_env: Vec<String>,
    /// Where it runs.
    #[serde(default)]
    pub working_dir: McpWorkingDir,
    /// How long it has to start.
    #[serde(default = "default_startup_timeout_secs")]
    pub startup_timeout_secs: u32,
    /// Only these tools are offered, when given.
    #[serde(default)]
    pub tools_allow: Option<Vec<String>>,
    /// These tools are left out.
    #[serde(default)]
    pub tools_deny: Option<Vec<String>>,
    /// What has to be there before it runs.
    #[serde(default)]
    pub requires: McpRequires,
}

/// Authentication supported by the MCP host, with no credential value in the public definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum McpAuthentication {
    /// A bearer token stored under this name in the server's declared secret variables.
    Bearer { secret: String },
}

/// How long an MCP server has, by default, to start: long enough for a package runner to fetch a server
/// the first time it is used.
pub const DEFAULT_STARTUP_TIMEOUT_SECS: u32 = 30;

fn default_startup_timeout_secs() -> u32 {
    DEFAULT_STARTUP_TIMEOUT_SECS
}

/// A definition read out of a harness's configuration: the server's spec, and the values of the variables
/// that `spec.secret_env` names. It stays on the machine it was read on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpDefinition {
    /// How to start or reach the server.
    pub spec: McpServerSpec,
    /// The values of its secret variables, by name.
    #[serde(default)]
    pub secrets: BTreeMap<String, Secret>,
}

impl McpDefinition {
    /// The declared transport, command/address and variable names identify a definition without comparing credentials.
    #[must_use]
    pub fn definition_fingerprint(&self) -> Hash {
        let names = self
            .spec
            .env
            .keys()
            .chain(self.spec.secret_env.iter())
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let shape = serde_json::json!({"transport":self.spec.transport,"command":self.spec.command,"url":self.spec.url,"environment":names});
        Hash::of(shape.to_string().as_bytes())
    }
    /// A digest that changes when the definition does, secrets included, and reveals none of them.
    #[must_use]
    pub fn digest(&self) -> Hash {
        // The members of both are written in a fixed order (struct order, sorted maps), so the bytes of one
        // definition are always the same.
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        Hash::of(&bytes)
    }

    /// What the answer to an import says of it.
    #[must_use]
    pub fn imported(&self, tools: Option<i64>) -> McpImported {
        McpImported {
            definition_fingerprint: Some(self.definition_fingerprint()),
            transport: self.spec.transport,
            digest: self.digest().to_string(),
            tools,
        }
    }
}

/// What is wrong with a definition.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpFault {
    /// Authentication must belong to an HTTP server and name one of its declared secrets.
    #[error("HTTP authentication names a declared secret variable")]
    BadAuthentication,
    /// A stdio server has no program.
    #[error("a stdio server names the program to run")]
    NoCommand,
    /// An http server has no address.
    #[error("an http server names the address to reach")]
    NoUrl,
    /// A secret variable has no value.
    #[error("the secret variable {0:?} has no value")]
    SecretWithoutValue(String),
    /// A value is given for a variable that is not declared secret.
    #[error("a value is given for {0:?}, which the definition does not name as a secret variable")]
    ValueWithoutSecret(String),
}

impl McpDefinition {
    /// Checks that the definition is complete for its transport and that its secrets and the names that
    /// declare them agree. Every fault is returned.
    pub fn check(&self) -> Result<(), Vec<McpFault>> {
        let mut faults = Vec::new();
        if let Some(McpAuthentication::Bearer { secret }) = &self.spec.authentication
            && (self.spec.transport != McpTransport::Http || !self.spec.secret_env.contains(secret))
        {
            faults.push(McpFault::BadAuthentication);
        }
        match self.spec.transport {
            McpTransport::Stdio => {
                if self
                    .spec
                    .command
                    .as_ref()
                    .is_none_or(|argv| argv.first().is_none_or(String::is_empty))
                {
                    faults.push(McpFault::NoCommand);
                }
            }
            McpTransport::Http => {
                if self.spec.url.as_deref().is_none_or(str::is_empty) {
                    faults.push(McpFault::NoUrl);
                }
            }
        }
        faults.extend(
            self.spec
                .secret_env
                .iter()
                .filter(|name| !self.secrets.contains_key(*name))
                .map(|name| McpFault::SecretWithoutValue(name.clone())),
        );
        faults.extend(
            self.secrets
                .keys()
                .filter(|name| !self.spec.secret_env.contains(name))
                .map(|name| McpFault::ValueWithoutSecret(name.clone())),
        );
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_list_names_a_model_by_its_name_by_a_family_or_by_a_group_it_is_in() {
        let list = |words: &[&str]| {
            words
                .iter()
                .map(|word| (*word).to_owned())
                .collect::<Vec<_>>()
        };
        let frontier = list(&["frontier"]);
        assert!(ModelPattern::list_names(
            &list(&["@frontier"]),
            "gpt-6-sol",
            &frontier
        ));
        assert!(ModelPattern::list_names(
            &list(&["@Frontier"]),
            "gpt-6-sol",
            &frontier
        ));
        assert!(
            !ModelPattern::list_names(&list(&["@frontier"]), "gpt-6-sol", &list(&["small"])),
            "the model is in another group"
        );
        assert!(
            !ModelPattern::list_names(&list(&["@frontier"]), "frontier", &[]),
            "a group is not a model's name, and a group no model is in names nothing"
        );
        assert!(ModelPattern::list_names(
            &list(&["claude-*", "@frontier"]),
            "openai/gpt-6-sol",
            &frontier
        ));
        assert!(ModelPattern::list_names(
            &list(&["gpt-6*"]),
            "openai/gpt-6-sol",
            &[]
        ));
        assert!(!ModelPattern::list_names(&[], "gpt-6-sol", &frontier));
        assert_eq!(ModelPattern::group_of("@small"), Some("small"));
        assert_eq!(ModelPattern::group_of("small"), None);
    }

    #[test]
    fn a_pattern_without_a_slash_names_the_family_through_any_gateway() {
        let family = ModelPattern::new("gpt-6*");
        assert!(family.matches("gpt-6-sol"));
        assert!(family.matches("openai/gpt-6-sol"));
        assert!(
            family.matches("OpenAI/GPT-6-Sol"),
            "both are read in lower case"
        );
        assert!(family.matches("openrouter/openai/gpt-6.1:free"));
        assert!(!family.matches("openai/gpt-5-sol"));
        assert!(
            !ModelPattern::new("gpt-6").matches("gpt-6-sol"),
            "a pattern without * names one model"
        );
        assert!(ModelPattern::new("gpt-6-sol").matches("openai/gpt-6-sol"));
    }

    #[test]
    fn a_pattern_with_a_slash_is_compared_with_the_whole_name() {
        let route = ModelPattern::new("openai/*");
        assert!(route.matches("openai/gpt-6-sol"));
        assert!(!route.matches("gpt-6-sol"));
        assert!(!route.matches("azure/openai/gpt-6-sol"));
        assert!(ModelPattern::new("*").matches("anything/at/all"));
        assert!(ModelPattern::new("*").matches(""));
    }

    #[test]
    fn the_pattern_with_the_most_characters_that_are_not_a_star_counts() {
        let card = [
            ("*", "any"),
            ("gpt-6*", "family"),
            ("gpt-6-sol", "exact"),
            ("claude*", "other"),
        ];
        assert_eq!(ModelPattern::best(card, "openai/gpt-6-sol"), Some("exact"));
        assert_eq!(ModelPattern::best(card, "gpt-6.1"), Some("family"));
        assert_eq!(ModelPattern::best(card, "mistral-large"), Some("any"));
        assert_eq!(ModelPattern::best([("claude*", 1)], "gpt-6"), None);
        assert_eq!(
            ModelPattern::best([("a*", 1), ("*a", 2)], "aa"),
            Some(1),
            "of equals the first counts"
        );
        assert_eq!(ModelPattern::new("g*t-*l").literal_len(), 4);
    }

    #[test]
    fn a_list_names_a_model_twice_and_a_gateway_may_state_prices() {
        let named = ModelSuggestion::named("openai/gpt-6-sol", Some("OpenAI: GPT-6 Sol"));
        assert_eq!(named.model_name, "openai/gpt-6-sol");
        assert_eq!(named.display_name, "OpenAI: GPT-6 Sol");
        assert_eq!(
            ModelSuggestion::named("gpt-6-sol", None).display_name,
            "gpt-6-sol"
        );
        assert_eq!(
            ModelSuggestion::named("gpt-6-sol", Some("")).display_name,
            "gpt-6-sol"
        );
        // The sample of a gateway's own list: dollars for one token, as strings.
        let pricing: GatewayPricing = serde_json::from_value(json!({
            "prompt": "0.0000001", "completion": "0.0000005",
            "input_cache_read": "0.00000001", "input_cache_write": "0.000000125", "web_search": "0.01"
        }))
        .unwrap();
        let rate = pricing_to_rate(&pricing, "1").unwrap();
        assert_eq!(rate.input_per_mtok, "0.1");
        assert_eq!(rate.output_per_mtok, "0.5");
        assert_eq!(rate.cached_input_per_mtok, "0.01");
        assert_eq!(rate.cache_write_per_mtok.as_deref(), Some("0.125"));
        let in_credits = pricing_to_rate(&pricing, "100").unwrap();
        assert_eq!(in_credits.input_per_mtok, "10");
        assert_eq!(in_credits.output_per_mtok, "50");
    }

    #[test]
    fn a_free_variant_is_rated_at_nothing_and_a_missing_cache_price_counts_as_input() {
        let free = GatewayPricing {
            prompt: "0".into(),
            completion: "0".into(),
            input_cache_read: None,
            input_cache_write: None,
        };
        let rate = pricing_to_rate(&free, "1").unwrap();
        assert_eq!(
            (rate.input_per_mtok.as_str(), rate.output_per_mtok.as_str()),
            ("0", "0")
        );
        let plain = GatewayPricing {
            prompt: "0.0000025".into(),
            completion: "0.00001".into(),
            input_cache_read: None,
            input_cache_write: None,
        };
        let rate = pricing_to_rate(&plain, "1").unwrap();
        assert_eq!(rate.input_per_mtok, "2.5");
        assert_eq!(rate.cached_input_per_mtok, "2.5");
        assert_eq!(rate.output_per_mtok, "10");
        assert!(rate.cache_write_per_mtok.is_none());
    }

    #[test]
    fn prices_that_are_not_plain_decimals_are_refused() {
        for bad in ["", ".", "-1", "1e-7", "0.0000001 ", "abc"] {
            assert_eq!(per_token_to_per_mtok(bad, "1"), None, "{bad:?}");
        }
        assert_eq!(per_token_to_per_mtok("0.1", "x"), None);
        assert_eq!(per_token_to_per_mtok("2", "1"), Some("2000000".into()));
        assert_eq!(
            per_token_to_per_mtok("0.0000000001", "1"),
            Some("0.0001".into())
        );
    }

    fn definition() -> McpDefinition {
        serde_json::from_value(json!({
            "spec": { "transport": "stdio", "command": ["tmux-mcp-rs"], "url": null, "env": { "LOG": "info" },
                      "secret_env": ["API_TOKEN"], "working_dir": "workspace", "startup_timeout_secs": 30,
                      "tools_allow": null, "tools_deny": null, "requires": { "commands": [], "capabilities": [] } },
            "secrets": { "API_TOKEN": "t-123" }
        }))
        .unwrap()
    }

    #[test]
    fn structural_identity_ignores_values_and_secrets_but_checks_command_address_and_variable_names()
     {
        let one = definition();
        let mut changed = definition();
        changed.spec.env.insert("LOG".into(), "debug".into());
        changed
            .secrets
            .insert("API_TOKEN".into(), Secret::new("replacement"));
        assert_eq!(
            one.definition_fingerprint(),
            changed.definition_fingerprint()
        );
        changed.spec.command = Some(vec!["different-server".into()]);
        assert_ne!(
            one.definition_fingerprint(),
            changed.definition_fingerprint()
        );
        let mut changed = definition();
        changed.spec.env.insert("ANOTHER".into(), "x".into());
        assert_ne!(
            one.definition_fingerprint(),
            changed.definition_fingerprint()
        );
        let mut changed = definition();
        changed.spec.secret_env.push("ANOTHER_SECRET".into());
        assert_ne!(
            one.definition_fingerprint(),
            changed.definition_fingerprint()
        );
        let mut http = definition();
        http.spec.transport = McpTransport::Http;
        http.spec.command = None;
        http.spec.url = Some("https://one.example.test/mcp".into());
        let mut other = http.clone();
        other.spec.url = Some("https://two.example.test/mcp".into());
        assert_ne!(one.definition_fingerprint(), http.definition_fingerprint());
        assert_ne!(
            http.definition_fingerprint(),
            other.definition_fingerprint()
        );
    }

    #[test]
    fn a_found_server_is_reported_by_name_alone() {
        let found: McpFound = serde_json::from_value(json!({
            "name": "github", "source": "codex", "scope": "user", "directory": null,
            "transport": "stdio", "digest": "9f2c"
        }))
        .unwrap();
        assert_eq!(found.scope, McpScope::User);
        let written = serde_json::to_value(&found).unwrap();
        for forbidden in ["command", "url", "env", "secrets", "token"] {
            assert!(written.get(forbidden).is_none(), "{forbidden}");
        }
    }

    #[test]
    fn a_definition_has_a_digest_that_changes_with_it_and_shows_none_of_its_secrets() {
        let one = definition();
        let mut changed = definition();
        changed
            .secrets
            .insert("API_TOKEN".into(), Secret::new("t-456"));
        assert_ne!(one.digest(), changed.digest());
        assert_eq!(one.digest(), definition().digest());
        assert!(!one.digest().to_string().contains("t-123"));
        assert!(!format!("{one:?}").contains("t-123"));
        let imported = one.imported(Some(12));
        assert_eq!(imported.transport, McpTransport::Stdio);
        assert_eq!(imported.tools, Some(12));
    }

    #[test]
    fn a_definition_is_complete_for_its_transport_and_its_secrets_agree_with_their_names() {
        assert_eq!(definition().check(), Ok(()));
        let mut broken = definition();
        broken.spec.command = None;
        broken.spec.secret_env.push("OTHER".into());
        broken.secrets.insert("EXTRA".into(), Secret::new("x"));
        let faults = broken.check().unwrap_err();
        assert!(faults.contains(&McpFault::NoCommand));
        assert!(faults.contains(&McpFault::SecretWithoutValue("OTHER".into())));
        assert!(faults.contains(&McpFault::ValueWithoutSecret("EXTRA".into())));
        let http: McpDefinition =
            serde_json::from_value(json!({ "spec": { "transport": "http" } })).unwrap();
        assert_eq!(http.check(), Err(vec![McpFault::NoUrl]));
        assert_eq!(http.spec.startup_timeout_secs, DEFAULT_STARTUP_TIMEOUT_SECS);
    }

    #[test]
    fn http_authentication_names_a_declared_secret_and_never_puts_its_value_in_the_spec() {
        let mut server = definition();
        assert_eq!(
            server.spec.authentication, None,
            "existing definitions need no new field"
        );
        server.spec.authentication = Some(McpAuthentication::Bearer {
            secret: "API_TOKEN".into(),
        });
        assert!(
            server
                .check()
                .unwrap_err()
                .contains(&McpFault::BadAuthentication),
            "a command uses secret variables, not HTTP authentication"
        );
        server.spec.transport = McpTransport::Http;
        server.spec.url = Some("https://tools.example.test/mcp".into());
        assert_eq!(server.check(), Ok(()));
        let public = serde_json::to_value(&server.spec).unwrap();
        assert_eq!(
            public["authentication"],
            json!({"kind":"bearer","secret":"API_TOKEN"})
        );
        assert!(!public.to_string().contains("t-123"));
        server.spec.secret_env.clear();
        assert!(
            server
                .check()
                .unwrap_err()
                .contains(&McpFault::BadAuthentication)
        );
    }

    #[test]
    fn a_session_the_harness_has_reads_as_the_api_shows_it() {
        let session: NativeSession = serde_json::from_value(json!({
            "native_session_ref": "0199abcd-aaaa", "title": "Fix login", "cwd": "/workspace/web",
            "updated_at": "2026-10-10T09:00:00.000Z", "adopted_thread_id": null
        }))
        .unwrap();
        assert!(session.adopted_thread_id.is_none());
    }

    fn said(turn: i64, text: &str) -> RecordedEntry {
        RecordedEntry::Said(RecordedWords {
            native_ref: format!("said:{turn}"),
            turn,
            text: text.to_owned(),
            left_out: 0,
            at: None,
        })
    }

    fn answered(turn: i64, text: &str) -> RecordedEntry {
        RecordedEntry::Item(RecordedItem {
            native_ref: format!("msg-{turn}"),
            turn,
            stream: crate::events::MAIN_STREAM.to_owned(),
            parent_native_ref: None,
            status: crate::events::ItemStatus::Completed,
            content: crate::events::ItemContent::AgentMessage(crate::events::AgentMessagePayload {
                text: text.to_owned(),
                text_len: i64::try_from(text.chars().count()).unwrap(),
                phase: None,
            }),
            at: None,
        })
    }

    #[test]
    fn a_record_reads_back_as_it_was_written_a_person_s_words_and_the_agent_s_items_in_order() {
        let turns = vec![
            vec![said(1, "Fix the login"), answered(1, "Fixed.")],
            vec![said(2, "Thanks")],
        ];
        let (text, kept, entries, left_out) = record_lines(&turns, RECORD_MAX_BYTES).unwrap();
        assert_eq!((kept, entries, left_out), (2, 3, 0));
        let first: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["entry"], "said", "{first}");
        assert_eq!(first["text"], "Fix the login");
        let second: serde_json::Value = serde_json::from_str(text.lines().nth(1).unwrap()).unwrap();
        assert_eq!(second["content"]["kind"], "agent_message", "{second}");
        let (read, unreadable) = read_record(&format!("{text}not an entry\n"));
        assert_eq!(unreadable, 1);
        assert_eq!(read, turns.concat());
    }

    #[test]
    fn a_record_larger_than_its_bound_keeps_its_latest_turns_whole_and_says_how_many_were_left_out()
    {
        let turns: Vec<Vec<RecordedEntry>> = (1..=10)
            .map(|turn| vec![said(turn, &"words ".repeat(20)), answered(turn, "done")])
            .collect();
        let (last_three, ..) = record_lines(&turns[7..], RECORD_MAX_BYTES).unwrap();
        let (text, kept, entries, left_out) = record_lines(&turns, last_three.len() + 1).unwrap();
        assert_eq!((kept, entries, left_out), (3, 6, 7));
        let (read, _) = read_record(&text);
        assert_eq!(
            read.first().map(RecordedEntry::turn),
            Some(8),
            "the latest turns are kept"
        );
        assert_eq!(read.last().map(RecordedEntry::turn), Some(10));
    }

    #[test]
    fn a_streamed_record_reports_ten_billion_attachments_as_one_aggregate() {
        const ATTACHMENTS: i64 = 10_000_000_000;
        let mut report = RecordReporter::default();
        report.push(&RecordedEntry::Said(RecordedWords {
            native_ref: "said:large".into(),
            turn: 1,
            text: "words".into(),
            left_out: ATTACHMENTS,
            at: None,
        }));
        let report = report.finish(0);
        assert_eq!(report.items_total, ATTACHMENTS + 1);
        assert_eq!(report.by_kind["attachment"].dropped, ATTACHMENTS);
        assert_eq!(report.by_kind["user_message"].kept, 1);
        assert_eq!(report.dropped.len(), 1);
        assert_eq!(report.dropped[0].count, ATTACHMENTS);
    }
}
