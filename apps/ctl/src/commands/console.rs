//! What a command says to the person who ran it: a line on standard output, a note, and the one line of a refusal with the status it leaves.
//!
//! Output is written with `writeln!` and its failure handled, never with
//! `println!`: that panics when the other end of a pipe is closed
//! (`orchctl api GET /me | head -1`), and a person is never shown the text of
//! a panic for something they did. A refusal is one line that begins
//! `orchctl:`, says what went wrong and what to do about it, and leaves the
//! status 1.

use std::fmt::Display;
use std::io::{self, Write};
use std::process::ExitCode;

/// What every line this program says about itself begins with.
const PROGRAM: &str = "orchctl";

/// Writes `line` and a line break on standard output.
pub fn say(line: impl Display) -> io::Result<()> {
    writeln!(io::stdout().lock(), "{line}")
}

/// Writes a line on standard error that is no refusal: something the person
/// should know and that does not stop the command. It is one line whatever
/// the message holds: what a far side answered may run over several.
pub fn note(line: impl Display) {
    let text = line.to_string();
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // A closed standard error has nobody to tell.
    let _ = writeln!(io::stderr().lock(), "{PROGRAM}: {one_line}");
}

/// Ends a command with a refusal: its line on standard error, and the status
/// that every failure of this program leaves.
#[must_use]
pub fn refuse(message: impl Display) -> ExitCode {
    note(message);
    ExitCode::FAILURE
}

/// Ends a command that either did what was asked or refused.
#[must_use]
pub fn finish<E: Display>(result: Result<(), E>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => refuse(message),
    }
}
