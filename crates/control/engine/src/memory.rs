//! Where an agent's notes are kept (its thread's directory) and by whom (its harness, or the agent with its tools), said to the adapter at each session; taken along by a switch, and by a fork that takes history or asks for them; nothing where the setting is off.
//!
//! The notes are files: `.orch/memory` of the thread's directory holds an index and a note to a file, so
//! that what the agent will take for granted next time is on the screen and lasts as long as the work does.
//! Who writes them, and what is said to the adapter about them at each session, is told by
//! [`orch_agent_contract::session::MemoryPlan`] and sent with the session. This module is the rule for what a
//! change does with them.
//!
//! **A change that leaves the directory where it is** does nothing: the two threads that share it share
//! its notes. **One that lands in another directory** carries them whatever files were chosen, unless the
//! request says not to; a fork that takes no history leaves them, since a conversation that knows nothing
//! of the work is not to learn from it by the back door. A directory that has notes already keeps its own,
//! and nothing is copied over them: the thread works with the notes of the directory it joins. Where
//! `agent.memory` is off, nothing is said about notes and nothing is done with them: they are files like
//! any other.

use orch_agent_contract::session::MemorySetting;

/// Where the notes are, below a thread's directory.
pub const DIRECTORY: &str = ".orch/memory";

/// Whether the notes of the source go along, when the directory is another: the request's word where it
/// has one, else yes for a switch and for a fork that takes history, and no for a fork that takes none.
#[must_use]
pub fn carried(asked: Option<bool>, switch: bool, takes_history: bool) -> bool {
    asked.unwrap_or(switch || takes_history)
}

/// Whether notes are kept at all for a thread, by the setting of its place.
#[must_use]
pub const fn kept(setting: MemorySetting) -> bool {
    !matches!(setting, MemorySetting::Off)
}

/// The notes below `directory`, as a path of the environment's workspace.
#[must_use]
pub fn below(directory: &str) -> String {
    let directory = directory.trim_matches('/');
    if directory.is_empty() {
        DIRECTORY.to_owned()
    } else {
        format!("{directory}/{DIRECTORY}")
    }
}

/// Whether a path, relative to a thread's directory, is among its notes.
#[must_use]
pub fn is_note(relative: &str) -> bool {
    relative == DIRECTORY || relative.starts_with(&format!("{DIRECTORY}/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_switch_and_a_fork_that_takes_history_carry_the_notes_and_a_fork_without_history_leaves_them()
     {
        assert!(carried(None, true, true));
        assert!(carried(None, false, true));
        assert!(!carried(None, false, false));
        assert!(
            !carried(Some(false), true, true),
            "the request has the last word"
        );
        assert!(carried(Some(true), false, false));
    }

    #[test]
    fn the_notes_are_found_below_a_directory_and_only_there() {
        assert_eq!(below("web"), "web/.orch/memory");
        assert_eq!(below(""), ".orch/memory");
        assert!(is_note(".orch/memory/MEMORY.md"));
        assert!(is_note(".orch/memory"));
        assert!(!is_note(".orch/memory-old/x"));
        assert!(!is_note("src/main.rs"));
        assert!(kept(MemorySetting::On) && kept(MemorySetting::Harness));
        assert!(!kept(MemorySetting::Off));
    }
}
