//! The check that shared code names no agent program and no vendor of one: a program is a manifest and an adapter, and everything else reads them.
//!
//! The check reads the Rust files of `crates/` and `apps/` and looks for the names of the programs Orch ships and of
//! their vendors in what the code says: the identifiers and the string literals. Comments are not read: they
//! explain, and a name there changes no behaviour. The places where a name belongs are listed here, each with the
//! reason; a name anywhere else fails the build.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// The names shared code must not say, in lower case: each is found as a part of an identifier or of a string.
const NAMES: [&str; 5] = ["codex", "claude", "openai", "anthropic", "deepseek"];

/// A short name that is a program only as a whole word, so it is looked for as one.
const WORDS: [&str; 1] = ["dsh"];

/// Wire protocols carry a vendor's name as their identifier. They are the names of formats an extension
/// implements, written by the code that relays them, and are not read as the vendor's.
const PROTOCOL_IDS: [&str; 4] = [
    "openai-responses",
    "openai-chat",
    "openai-embeddings",
    "anthropic-messages",
];

/// Directories where names belong, with why.
const ALLOWED_DIRECTORIES: [(&str, &str); 4] = [
    (
        "crates/contracts/agent-programs/",
        "the manifests and the catalogue of the programs: data that names them",
    ),
    (
        "crates/tooling/",
        "the test kit, the stress rig and these tools are not shipped code of the product",
    ),
    (
        "crates/contracts/provider/src/",
        "the sources of a key's facts are named by the provider they read, which is catalogue data",
    ),
    (
        "crates/platform/conformance/src/provider/",
        "conformance examples of the provider sources",
    ),
];

/// Files where names belong, with why: the composition roots (which choose what is built in), the settings examples
/// and the catalogue the database is seeded from.
const ALLOWED_FILES: [(&str, &str); 11] = [
    (
        "apps/connector/src/composition.rs",
        "the connector's composition root chooses its adapters",
    ),
    (
        "apps/api/src/composition.rs",
        "the API's composition root chooses its sources",
    ),
    (
        "apps/node/src/conformance.rs",
        "the node's composition root makes the adapter that a check drives",
    ),
    ("apps/node/src/docker.rs", "the node's composition root"),
    (
        "crates/contracts/settings/src/examples.rs",
        "examples in the settings' help are data that names real programs",
    ),
    (
        "crates/control/db/src/seeds.rs",
        "the catalogue the system is seeded from, with the presets of API providers",
    ),
    (
        "crates/contracts/model/src/protocol.rs",
        "the wire protocols are implemented by extensions and named by their vendors' formats",
    ),
    (
        "crates/runtime/connector/src/forwarder.rs",
        "the forwarder relays the wire protocols, each by its own format",
    ),
    (
        "crates/control/api/src/routes/resources/upstreams.rs",
        "the titles of the wire protocols an upstream may speak",
    ),
    (
        "crates/control/api/src/routes/resources/setup.rs",
        "the titles of the wire protocols and the presets of API providers",
    ),
    (
        "crates/control/engine/src/summaries/completion.rs",
        "the request shapes of the wire protocols a summary may be asked in",
    ),
];

/// A name found where it does not belong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// The file, relative to the root.
    pub path: PathBuf,
    /// The line.
    pub line: usize,
    /// The name found.
    pub name: String,
    /// The identifier or string that holds it, cut short.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Ident,
    Text,
    Punct,
}

#[derive(Debug, Clone)]
struct Token {
    kind: Kind,
    text: String,
    line: usize,
}

/// The identifiers, strings and punctuation of Rust source, without comments, characters and lifetimes.
fn tokens(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let (mut at, mut line) = (0, 1);
    while at < chars.len() {
        let c = chars[at];
        if c == '\n' {
            line += 1;
            at += 1;
        } else if c.is_whitespace() {
            at += 1;
        } else if c == '/' && chars.get(at + 1) == Some(&'/') {
            while at < chars.len() && chars[at] != '\n' {
                at += 1;
            }
        } else if c == '/' && chars.get(at + 1) == Some(&'*') {
            let mut depth = 0;
            while at < chars.len() {
                if chars[at] == '/' && chars.get(at + 1) == Some(&'*') {
                    depth += 1;
                    at += 2;
                } else if chars[at] == '*' && chars.get(at + 1) == Some(&'/') {
                    depth -= 1;
                    at += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    if chars[at] == '\n' {
                        line += 1;
                    }
                    at += 1;
                }
            }
        } else if let Some((end, hashes)) = raw_string(&chars, at) {
            let start = at;
            let body: String = chars[start..end].iter().collect();
            let _ = hashes;
            line += body.matches('\n').count();
            out.push(Token {
                kind: Kind::Text,
                text: body,
                line,
            });
            at = end;
        } else if c == '"' {
            let start = at;
            at += 1;
            while at < chars.len() && chars[at] != '"' {
                at += if chars[at] == '\\' { 2 } else { 1 };
            }
            at = (at + 1).min(chars.len());
            let body: String = chars[start..at].iter().collect();
            let lines = body.matches('\n').count();
            out.push(Token {
                kind: Kind::Text,
                text: body,
                line,
            });
            line += lines;
        } else if c == '\'' {
            // A character literal, or a lifetime: neither names a program.
            let rest = &chars[at + 1..];
            let closes = if rest.first() == Some(&'\\') {
                rest.iter()
                    .skip(2)
                    .position(|each| *each == '\'')
                    .map(|n| n + 3)
            } else if rest.get(1) == Some(&'\'') {
                Some(2)
            } else {
                None
            };
            at += 1 + closes.unwrap_or(0);
        } else if c.is_alphabetic() || c == '_' {
            let start = at;
            while at < chars.len() && (chars[at].is_alphanumeric() || chars[at] == '_') {
                at += 1;
            }
            out.push(Token {
                kind: Kind::Ident,
                text: chars[start..at].iter().collect(),
                line,
            });
        } else {
            out.push(Token {
                kind: Kind::Punct,
                text: c.to_string(),
                line,
            });
            at += 1;
        }
    }
    out
}

/// Where a raw string that begins at `at` ends, and how many `#` it has.
fn raw_string(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let mut at_hashes = at;
    if chars.get(at_hashes) == Some(&'b') {
        at_hashes += 1;
    }
    if chars.get(at_hashes) != Some(&'r') {
        return None;
    }
    let mut cursor = at_hashes + 1;
    let mut hashes = 0;
    while chars.get(cursor) == Some(&'#') {
        hashes += 1;
        cursor += 1;
    }
    if chars.get(cursor) != Some(&'"') {
        return None;
    }
    cursor += 1;
    while cursor < chars.len() {
        if chars[cursor] == '"'
            && (0..hashes).all(|offset| chars.get(cursor + 1 + offset) == Some(&'#'))
        {
            return Some((cursor + 1 + hashes, hashes));
        }
        cursor += 1;
    }
    Some((chars.len(), hashes))
}

/// The tokens of the code outside the modules that only a test builds.
fn shipped(tokens: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        if is_test_attribute(&tokens, at) {
            // Skip the attributes and the item after them: a module's braces, or up to the next semicolon.
            let mut cursor = at;
            while cursor < tokens.len() && tokens[cursor].text != "{" && tokens[cursor].text != ";"
            {
                cursor += 1;
            }
            if tokens.get(cursor).is_some_and(|token| token.text == "{") {
                let mut depth = 0;
                while cursor < tokens.len() {
                    match tokens[cursor].text.as_str() {
                        "{" if tokens[cursor].kind == Kind::Punct => depth += 1,
                        "}" if tokens[cursor].kind == Kind::Punct => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    cursor += 1;
                }
            }
            at = cursor + 1;
        } else {
            out.push(tokens[at].clone());
            at += 1;
        }
    }
    out
}

/// Whether `#[cfg(test)]` stands at `at`.
fn is_test_attribute(tokens: &[Token], at: usize) -> bool {
    let words = ["#", "[", "cfg", "(", "test", ")", "]"];
    words.iter().enumerate().all(|(offset, word)| {
        tokens
            .get(at + offset)
            .is_some_and(|token| token.text == *word)
    })
}

/// The name that `text` says, if it says one of those that shared code must not.
fn named(text: &str, kind: &Kind) -> Option<&'static str> {
    let mut lower = text.to_ascii_lowercase();
    for protocol in PROTOCOL_IDS {
        lower = lower.replace(protocol, "");
    }
    if let Some(name) = NAMES.iter().copied().find(|name| lower.contains(*name)) {
        return Some(name);
    }
    WORDS.iter().copied().find(|word| {
        lower
            .split(|each: char| !each.is_ascii_alphanumeric())
            .any(|part| part == *word)
            || (*kind == Kind::Ident && lower.split('_').any(|part| part == *word))
    })
}

/// Whether the file at `path` (relative to the root) is a test, or builds only tests, or belongs where names do.
fn exempt(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.split('/')
        .any(|part| part == "tests" || part == "benches")
        || name == "tests.rs"
        || name.ends_with("_tests.rs")
        || name == "test_common.rs"
        || name == "testing.rs"
        || ALLOWED_DIRECTORIES
            .iter()
            .any(|(directory, _)| path.starts_with(directory))
        || ALLOWED_FILES.iter().any(|(file, _)| path == *file)
}

/// Every name that does not belong, in the Rust files of `crates/` and `apps/` under `root`.
pub fn scan(root: &Path) -> Vec<Violation> {
    let mut found = Vec::new();
    for base in ["crates", "apps"] {
        for entry in WalkDir::new(root.join(base))
            .into_iter()
            .filter_entry(|entry| entry.file_name() != "target")
            .flatten()
        {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "rs") {
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let relative_text = relative.to_string_lossy().replace('\\', "/");
            if exempt(&relative_text) {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(path) else {
                continue;
            };
            for token in shipped(tokens(&source)) {
                if token.kind == Kind::Punct {
                    continue;
                }
                if let Some(name) = named(&token.text, &token.kind) {
                    found.push(Violation {
                        path: relative.to_owned(),
                        line: token.line,
                        name: name.to_owned(),
                        text: token.text.chars().take(60).collect(),
                    });
                }
            }
        }
    }
    found
}

/// Runs the check: the names that do not belong are printed, and any of them fails it.
pub fn run(root: &Path) -> Result<(), String> {
    let violations = scan(root);
    if violations.is_empty() {
        eprintln!("no agent program or vendor is named in shared code");
        return Ok(());
    }
    for violation in &violations {
        eprintln!(
            "{}:{}: names {} in {}",
            violation.path.display(),
            violation.line,
            violation.name,
            violation.text
        );
    }
    Err(format!(
        "{} names of agent programs or vendors stand in shared code; a program is a manifest, and the code reads it",
        violations.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(source: &str) -> Vec<String> {
        shipped(tokens(source))
            .into_iter()
            .filter(|token| token.kind != Kind::Punct)
            .filter_map(|token| {
                named(&token.text, &token.kind).map(|name| format!("{name}@{}", token.line))
            })
            .collect()
    }

    #[test]
    fn a_name_in_code_is_found_and_a_name_in_a_comment_is_not() {
        assert_eq!(
            found("// Codex says so\nlet kind = \"codex\";"),
            ["codex@2"]
        );
        assert_eq!(
            found("/* Claude\nCode */ fn claude_code() {}"),
            ["claude@2"]
        );
        assert_eq!(
            found("/// Anthropic's format\nfn plain() {}"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_protocol_is_not_a_vendor_and_a_test_module_is_not_shipped() {
        assert!(found("const P: &str = \"openai-responses\";").is_empty());
        assert_eq!(
            found("const P: &str = \"openai-responses and openai\";"),
            ["openai@1"]
        );
        assert!(
            found("fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() { let _ = \"codex\"; }\n}\nfn c() {}").is_empty()
        );
        assert_eq!(
            found("#[cfg(test)]\nmod tests { fn b() {} }\nfn c() { let _ = \"claude\"; }"),
            ["claude@3"]
        );
    }

    #[test]
    fn a_raw_string_a_character_and_a_lifetime_are_read_as_what_they_are() {
        assert_eq!(found("let a = r#\"a \"codex\" b\"#;"), ["codex@1"]);
        assert!(found("fn f<'a>(x: &'a str) -> char { 'c' }").is_empty());
        assert_eq!(found("let c = '\\''; let s = \"claude\";"), ["claude@1"]);
    }

    #[test]
    fn a_short_name_counts_only_as_a_whole_word() {
        assert_eq!(found("let a = \"run dsh now\";"), ["dsh@1"]);
        assert!(found("let a = \"adshield\";").is_empty());
        assert_eq!(found("fn dsh_home() {}"), ["dsh@1"]);
    }

    #[test]
    fn tests_and_composition_roots_are_exempt_and_other_files_are_not() {
        assert!(exempt("crates/control/api/tests/agents.rs"));
        assert!(exempt("apps/connector/src/composition.rs"));
        assert!(exempt("crates/contracts/agent-programs/src/registry.rs"));
        assert!(!exempt("crates/control/engine/src/turns.rs"));
    }

    #[test]
    fn the_tree_of_the_product_names_no_program_in_shared_code() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .map(Path::to_owned)
            .unwrap_or_default();
        let violations = scan(&root);
        assert!(
            violations.is_empty(),
            "names of agent programs or vendors in shared code: {violations:#?}"
        );
    }
}
