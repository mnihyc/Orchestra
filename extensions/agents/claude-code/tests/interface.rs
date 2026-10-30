//! The flags the adapter passes to Claude Code are the flags every build is checked for
//! (the interface of the manifest of the program), apart from the two its help does not list.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::BTreeSet;
use std::path::Path;

use orch_agent_programs::manifest::Need;

/// Flags the adapter passes that Claude Code's help does not list, so that no build can be checked for them; and
/// the flag of `claude auth status`, a command of its own.
const UNLISTED: [&str; 3] = [
    "--append-system-prompt-file",
    "--resume-session-at",
    "--json",
];

/// The code of every source file, without its tests.
fn code() -> String {
    let mut all = String::new();
    let mut pending = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                all.push_str(text.split("#[cfg(test)]").next().unwrap());
                all.push('\n');
            }
        }
    }
    all
}

/// Every flag a string literal of `code` begins with.
fn flags(code: &str) -> BTreeSet<String> {
    code.split('"')
        .skip(1)
        .step_by(2)
        .filter(|literal| {
            literal
                .strip_prefix("--")
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_alphabetic()))
        })
        .map(|literal| {
            literal
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
                .next()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[test]
fn every_flag_the_adapter_passes_is_checked_in_each_build_and_every_flag_checked_is_passed() {
    let code = code();
    let manifest = orch_adapter_claude_code::descriptor::manifest();
    let needs: &[Need] = &manifest.checks.interface.as_ref().unwrap().needs;
    let listed: BTreeSet<&str> = needs.iter().map(|need| need.name.as_str()).collect();
    let unchecked: Vec<String> = flags(&code)
        .into_iter()
        .filter(|flag| !listed.contains(flag.as_str()) && !UNLISTED.contains(&flag.as_str()))
        .collect();
    assert!(
        unchecked.is_empty(),
        "flags the adapter passes that no build is checked for: {unchecked:?}"
    );
    let unused: Vec<&str> = listed
        .iter()
        .filter(|flag| !code.contains(&format!("\"{flag}")))
        .copied()
        .collect();
    assert!(
        unused.is_empty(),
        "flags checked that the adapter does not pass: {unused:?}"
    );
}
