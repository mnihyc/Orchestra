//! Shared document organization uses one atomic domain for people and agents.
//!
//! Only destructive deletion has a preview and confirmation capability. A word binds the exact
//! arguments and the domain's complete membership/authority stamp for ten minutes. All other
//! commands execute directly. The domain rechecks every descendant, destination, revision,
//! collision and limit on the supplied transaction, with the actual calling agent's authority.
//! Copies, current content/files/history, provenance and durable replay have one implementation.

use crate::document_tree;
use crate::tools::definitions::{Code, Refusal};
use crate::tools::{Answer, tree};
use chrono::{DateTime, Duration, Utc};
use orch_types::{Id, Json};
use serde::Deserialize;
#[cfg(test)]
use serde_json::json;
use std::collections::HashMap;
#[cfg(test)]
use std::collections::HashSet;

mod containers;
pub(crate) use containers::domain_result;

/// Every affected document and folder consumes the same bounded operation budget.
pub const AFFECTS_MAX: usize = document_tree::ITEMS_MAX;
const PLANS_PER_THREAD_MAX: usize = 16;
const PLANS_MAX: usize = 4_096;
const ANSWERS_MAX: usize = 512;
/// A deletion capability contains 128 random bits, as before.
const WORD_BYTES: usize = 16;

// ---------------------------------------------------------------- what a call asks

/// What `manage_shared` is told to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    CreateFolder,
    Copy,
    Delete,
    Move,
    Restore,
}

impl Command {
    const fn word(self) -> &'static str {
        match self {
            Self::CreateFolder => "create_folder",
            Self::Copy => "copy",
            Self::Delete => "delete",
            Self::Move => "move",
            Self::Restore => "restore",
        }
    }
}

/// The arguments of `manage_shared`, as the schema lets them through.
#[derive(Debug, Deserialize)]
struct Arguments {
    command: String,
    paths: Vec<String>,
    to: Option<String>,
    confirm: Option<String>,
    name: Option<String>,
}

/// What a plan is for: the command, the paths and the place they go to, exactly as they were given. The word of a plan
/// is good for these and for no other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asked {
    command: Command,
    paths: Vec<String>,
    to: Option<String>,
    name: Option<String>,
}

/// Reads the arguments the schema let through, and checks what a schema cannot say: which command takes `to`.
fn asked_of(arguments: &Json) -> Result<(Asked, Option<String>), Refusal> {
    let given: Arguments = serde_json::from_value(arguments.clone())
        .map_err(|error| Refusal::invalid(error.to_string()))?;
    let command = match given.command.as_str() {
        "create_folder" => Command::CreateFolder,
        "copy" => Command::Copy,
        "delete" => Command::Delete,
        "move" => Command::Move,
        "restore" => Command::Restore,
        other => {
            return Err(Refusal::invalid(format!(
                "\"command\" is create_folder, copy, delete, move or restore; yours is \"{other}\"."
            )));
        }
    };
    let paths: Vec<String> = given
        .paths
        .iter()
        .map(|path| path.trim().to_owned())
        .collect();
    if let Some(at) = paths.iter().position(String::is_empty) {
        return Err(Refusal::invalid(format!(
            "\"paths\" names a document or document folder each, and entry {} is empty.",
            at + 1
        )));
    }
    if paths.is_empty() || paths.len() > crate::tools::definitions::MANAGE_PATHS_MAX {
        return Err(Refusal::invalid(format!(
            "paths must contain at least one and at most {} selections.",
            crate::tools::definitions::MANAGE_PATHS_MAX
        )));
    }
    let selections = paths
        .iter()
        .map(|path| {
            tree::parse_path(path).ok_or_else(|| {
                Refusal::invalid("A selected id must be a complete document or folder identifier.")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (at, selected) in selections.iter().enumerate() {
        if selections[..at]
            .iter()
            .any(|other| match (selected, other) {
                (tree::Named::Id(left), tree::Named::Id(right)) => left == right,
                (
                    tree::Named::Path {
                        absolute: left_root,
                        parts: left,
                    },
                    tree::Named::Path {
                        absolute: right_root,
                        parts: right,
                    },
                ) => {
                    left_root == right_root
                        && !left.iter().chain(right).any(|part| part == "..")
                        && (left.starts_with(right) || right.starts_with(left))
                }
                _ => false,
            })
        {
            return Err(Refusal::invalid(
                "Selections must not overlap. Select either a document folder or its descendants, and name each item once.",
            ));
        }
    }
    let to = given
        .to
        .as_deref()
        .map(str::trim)
        .filter(|to| !to.is_empty())
        .map(str::to_owned);
    match (command, &to) {
        (Command::Copy | Command::Move, None) => {
            return Err(Refusal::invalid(
                "copy or move needs \"to\": an existing writable document directory.",
            ));
        }
        (Command::CreateFolder | Command::Delete | Command::Restore, Some(_)) => {
            return Err(Refusal::invalid(format!(
                "\"to\" is only for copy and move; {} does not take it.",
                command.word()
            )));
        }
        _ => {}
    }
    if command != Command::Delete && given.confirm.is_some() {
        return Err(Refusal::invalid(
            "Only delete takes confirm. All other management commands execute directly.",
        ));
    }
    if command == Command::CreateFolder && paths.len() != 1 {
        return Err(Refusal::invalid(
            "create_folder takes exactly one new folder path.",
        ));
    }
    if given.name.is_some()
        && (!matches!(command, Command::Copy | Command::Move) || paths.len() != 1)
    {
        return Err(Refusal::invalid(
            "name is only for copy or move with exactly one source.",
        ));
    }
    if given.name.as_ref().is_some_and(|name| {
        name.is_empty()
            || name.trim() != name
            || name.contains(['/', '\\', '\0'])
            || matches!(name.as_str(), "." | "..")
    }) {
        return Err(Refusal::invalid(
            "name must be one nonempty basename, without separators or parent steps.",
        ));
    }
    Ok((
        Asked {
            command,
            paths,
            to,
            name: given.name,
        },
        given.confirm.map(|word| word.trim().to_owned()),
    ))
}

// ---------------------------------------------------------------- the plans that wait for their word

/// A plan that waits for its word.
#[derive(Debug, Clone)]
pub(crate) struct Waiting {
    made: DateTime<Utc>,
    asked: Asked,
    stamp: Option<document_tree::MembershipStamp>,
}

/// A call answered, kept so that the same call said twice is answered the same.
#[derive(Debug, Clone)]
struct Answered {
    at: DateTime<Utc>,
    asked: Asked,
    answer: Answer,
}

/// What a word comes to when it is used.
#[derive(Debug)]
pub(crate) enum Claim {
    /// No plan of this thread waits under it: it was made up, it was used, it is another thread's, or it is too old.
    Missing,
    /// A plan waits under it for other arguments. It stays, for the arguments it was made for.
    Other,
    /// The plan, which is spent.
    Claimed(Waiting),
}

/// The plans that agents were given a word for, and the answers to the calls that were made, for as long as a word is
/// good for (ten minutes, [`crate::config::MANAGE_PLAN_KEPT`]).
#[derive(Debug)]
pub struct Plans {
    kept: Duration,
    waiting: HashMap<(Id, String), Waiting>,
    answered: HashMap<(Id, String), Answered>,
}

impl Plans {
    /// No plan waits; a plan is kept for `kept`.
    #[must_use]
    pub fn new(kept: std::time::Duration) -> Self {
        Self {
            kept: Duration::from_std(kept).unwrap_or(Duration::MAX),
            waiting: HashMap::new(),
            answered: HashMap::new(),
        }
    }

    fn sweep(&mut self, now: DateTime<Utc>) {
        let kept = self.kept;
        self.waiting.retain(|_, each| now - each.made < kept);
        self.answered.retain(|_, each| now - each.at < kept);
    }

    /// Keeps a plan under its word. A thread has a few waiting at most, and all of them have a bound.
    pub(crate) fn keep(&mut self, thread: Id, word: &str, waiting: Waiting, now: DateTime<Utc>) {
        self.sweep(now);
        let of_thread: Vec<(DateTime<Utc>, String)> = self
            .waiting
            .iter()
            .filter(|((owner, _), _)| *owner == thread)
            .map(|((_, word), each)| (each.made, word.clone()))
            .collect();
        if of_thread.len() >= PLANS_PER_THREAD_MAX
            && let Some((_, oldest)) = of_thread.into_iter().min()
        {
            self.waiting.remove(&(thread, oldest));
        }
        if self.waiting.len() >= PLANS_MAX
            && let Some(oldest) = self
                .waiting
                .iter()
                .min_by_key(|(_, each)| each.made)
                .map(|(key, _)| key.clone())
        {
            self.waiting.remove(&oldest);
        }
        self.waiting.insert((thread, word.to_owned()), waiting);
    }

    /// Uses a word for these arguments. A plan is spent by the call that uses it, whatever that call comes to.
    pub(crate) fn claim(
        &mut self,
        thread: Id,
        word: &str,
        asked: &Asked,
        now: DateTime<Utc>,
    ) -> Claim {
        self.sweep(now);
        let key = (thread, word.to_owned());
        match self.waiting.get(&key) {
            None => Claim::Missing,
            Some(found) if found.asked != *asked => Claim::Other,
            Some(_) => self
                .waiting
                .remove(&key)
                .map_or(Claim::Missing, Claim::Claimed),
        }
    }

    /// The answer given to a call, if it was made already and its answer is still kept.
    fn replay(
        &mut self,
        thread: Id,
        call_ref: &str,
        asked: &Asked,
        now: DateTime<Utc>,
    ) -> Option<Answer> {
        self.sweep(now);
        self.answered
            .get(&(thread, call_ref.to_owned()))
            .map(|each| {
                if each.asked == *asked {
                    each.answer.clone()
                } else {
                    Refusal::invalid(
                        "This call reference was already used with different management arguments.",
                    )
                    .into()
                }
            })
    }

    /// Keeps the answer given to a call.
    fn remember(
        &mut self,
        thread: Id,
        call_ref: &str,
        asked: &Asked,
        now: DateTime<Utc>,
        answer: &Answer,
    ) {
        if self.answered.len() >= ANSWERS_MAX
            && let Some(oldest) = self
                .answered
                .iter()
                .min_by_key(|(_, each)| each.at)
                .map(|(key, _)| key.clone())
        {
            self.answered.remove(&oldest);
        }
        self.answered.insert(
            (thread, call_ref.to_owned()),
            Answered {
                at: now,
                asked: asked.clone(),
                answer: answer.clone(),
            },
        );
    }

    /// How many plans wait for their word under a thread.
    #[must_use]
    pub fn waiting_for(&self, thread: Id) -> usize {
        self.waiting
            .keys()
            .filter(|(owner, _)| *owner == thread)
            .count()
    }
}

/// A word: 128 random bits, written as 32 hexadecimal characters.
fn new_word() -> String {
    let bytes: [u8; WORD_BYTES] = rand::random();
    Id::from_bytes(bytes).simple().to_string()
}

#[cfg(test)]
mod tests;
