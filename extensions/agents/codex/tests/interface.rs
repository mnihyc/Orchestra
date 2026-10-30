//! The names the adapter uses of Codex's protocol are the names every build is checked for
//! (the interface of the manifest of the program): each method this adapter's code names is in the list, or is one it
//! opts out of, and each name in the list is one its code uses.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::collections::BTreeSet;
use std::path::Path;

use orch_agent_programs::manifest::Need;

/// The strings of the adapter's code that look like a method but are none: image types, and the name of Orch's
/// own export file.
const NOT_METHODS: [&str; 5] = [
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/webp",
    "orch/exports",
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

/// Every string literal of `code` shaped like a method: words joined by `/`.
fn methods(code: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for literal in code.split('"').skip(1).step_by(2) {
        let mut parts = literal.split('/');
        let first = parts.next().unwrap_or_default();
        let rest: Vec<_> = parts.collect();
        let word = |part: &str| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        if !rest.is_empty()
            && first.starts_with(|c: char| c.is_ascii_lowercase())
            && word(first)
            && rest.iter().all(|part| word(part))
        {
            found.insert(literal.to_owned());
        }
    }
    found
}

#[test]
fn every_method_the_adapter_names_is_checked_in_each_build_and_every_name_checked_is_used() {
    let code = code();
    let manifest = orch_adapter_codex::descriptor::manifest();
    let needs: &[Need] = &manifest.checks.interface.as_ref().unwrap().needs;
    let listed: BTreeSet<&str> = needs.iter().map(|need| need.name.as_str()).collect();
    let opted_out = code
        .split("const OPTED_OUT")
        .nth(1)
        .and_then(|rest| rest.split("];").next())
        .unwrap();
    let unchecked: Vec<String> = methods(&code)
        .into_iter()
        .filter(|method| {
            !listed.contains(method.as_str())
                && !NOT_METHODS.contains(&method.as_str())
                && !opted_out.contains(&format!("\"{method}\""))
        })
        .collect();
    assert!(
        unchecked.is_empty(),
        "methods the adapter uses that no build is checked for: {unchecked:?}"
    );
    let unused: Vec<&str> = listed
        .iter()
        .filter(|name| {
            let named = name.rsplit_once('.').map_or(**name, |(_, member)| member);
            !code.contains(&format!("\"{named}\""))
        })
        .copied()
        .collect();
    assert!(
        unused.is_empty(),
        "names checked that the adapter does not use: {unused:?}"
    );
}
