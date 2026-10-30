//! References an agent writes and reads: one grammar for a shared-tree path or an identifier with a part, where one stands in Markdown, and the forms a reference is written in.
//!
//! A reference names a thing of the shared tree, and may name one of its parts: a block of a document, a turn of a
//! conversation, a message of a chat, numbered as `read_shared` numbers them (the number it takes as `from`). An
//! agent writes one as the destination of a Markdown link, `[Rollout plan](docs/Rollout plan.md#3)`, with angle
//! brackets round a destination that holds a space or a parenthesis, `[Rollout plan](<docs/Rollout plan.md#3>)`,
//! or in backquotes, `` `docs/Rollout plan.md#3` ``. The path is one of the tree, from `/` or from the agent's own
//! topic, or `id:` and an identifier, which no rename changes.
//!
//! Orch keeps a reference typed ([`Typed`]): what kind of thing, which one, and which part; a block by its
//! identifier, so that writing above it moves nothing, and a turn or a message by its number, which never changes.
//! In a document it is an inline reference; in the text of a message it is a link to the reference's address
//! ([`address`]), which is also what an export writes. Each reader is given it in their own terms: a person a chip
//! with the thing's current name, an agent `[name](path#n)` with its own path to the thing ([`agent_link`]), and a
//! reader who may not read the thing words that name nothing, `(unavailable reference 1)` ([`hidden`]), numbered so
//! that a change written around such words keeps the reference they stand for.

use orch_api_contract::documents::{InlineRefKind, RefPart};
use orch_types::Id;
use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
use std::ops::Range;

/// What a reference by identifier begins with.
pub const ID_PREFIX: &str = "id:";

/// The words that stand for a reference its reader may not read, before its number.
pub const HIDDEN_WORDS: &str = "unavailable reference";

/// The beginnings of addresses that lead out of the shared tree: a link or a text in backquotes that begins so is no
/// reference.
const SCHEMES: [&str; 8] = [
    "http:",
    "https:",
    "mailto:",
    "ftp:",
    "file:",
    "data:",
    "javascript:",
    "tel:",
];

/// Where the address of a node, a document and a chat begins.
const NODES: &str = "/nodes/";
const DOCUMENTS: &str = "/documents/";
const CHATS: &str = "/chats/";
const FILES: &str = "/files/";
const ENVIRONMENTS: &str = "/environments/";

/// A reference as an agent writes it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Written {
    /// A path of the shared tree, from `/` or from the agent's own topic, or `id:` and an identifier.
    pub path: String,
    /// The part it names, in the thing's own unit, counted from 1; none for the thing whole.
    pub part: Option<i64>,
}

/// A reference as Orch keeps it: what kind of thing, which one, and which part.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Typed {
    /// What kind of thing.
    pub kind: InlineRefKind,
    /// The node, the document or the chat.
    pub id: Id,
    /// A workspace-relative path for an environment file; absent for other kinds.
    pub path: Option<String>,
    /// The block, the turn or the message; none for the thing whole.
    pub part: Option<RefPart>,
}

/// Where a reference was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// As the destination of a link.
    Link,
    /// As a text in backquotes.
    Code,
}

/// A reference found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// Where it stands in the text: the whole link, or the backquotes with what is in them.
    pub span: Range<usize>,
    /// The destination or the text in backquotes, as written.
    pub text: String,
    /// The words of a link; for a text in backquotes, that text.
    pub label: String,
    /// How it was written.
    pub form: Form,
}

/// How one reader is given a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Writing {
    /// A link with the thing's name, the reader's own path to it, and the number of its part there.
    Link {
        /// The thing's name now.
        name: String,
        /// The reader's path to it.
        path: String,
        /// The number of the part, where the reference names one that is there.
        part: Option<i64>,
    },
    /// Words that name nothing, with the number they have among those of the text.
    Hidden(usize),
}

/// The options a text is read with: those of the one reader of Markdown, so that a reference is found where that
/// reader finds a link.
#[must_use]
pub fn options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_MATH);
    options.insert(Options::ENABLE_GFM);
    options
}

/// Reads a destination or a text in backquotes as a reference: none where it is empty, an anchor, an address of the
/// web or of mail, or the address of a typed reference (which is read by [`typed`]). A `#` followed by digits at
/// its end is the part; a name that ends so is tried whole by whoever resolves it.
#[must_use]
pub fn parse(text: &str) -> Option<Written> {
    let text = text.trim();
    if text.is_empty()
        || text.starts_with('#')
        || text.starts_with("//")
        || text.chars().any(char::is_control)
        || typed(text).is_some()
    {
        return None;
    }
    let lower = text.to_ascii_lowercase();
    if SCHEMES.iter().any(|scheme| lower.starts_with(scheme)) {
        return None;
    }
    if let Some((path, digits)) = text
        .rsplit_once('#')
        .filter(|(_, digits)| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
        .or_else(|| {
            text.rsplit_once(':').filter(|(_, digits)| {
                !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        && !path.is_empty()
    {
        return Some(Written {
            path: path.to_owned(),
            part: digits.parse().ok(),
        });
    }
    Some(Written {
        path: text.to_owned(),
        part: None,
    })
}

/// The address of a typed reference: what the text of a message keeps, and what an export writes.
#[must_use]
pub fn address(typed: &Typed) -> String {
    if typed.kind == InlineRefKind::EnvironmentFile {
        let path = encode_path(typed.path.as_deref().unwrap_or_default());
        let part = match typed.part {
            Some(RefPart::Line(line)) => format!("#line={line}"),
            _ => String::new(),
        };
        return format!("{ENVIRONMENTS}{}/files/{path}{part}", typed.id);
    }
    let base = match typed.kind {
        InlineRefKind::Node => NODES,
        InlineRefKind::Document => DOCUMENTS,
        InlineRefKind::Chat => CHATS,
        InlineRefKind::File => FILES,
        InlineRefKind::EnvironmentFile => ENVIRONMENTS,
    };
    let part = match &typed.part {
        None => String::new(),
        Some(RefPart::Block(block)) => format!("#block={block}"),
        Some(RefPart::Turn(turn)) => format!("#turn={turn}"),
        Some(RefPart::Message(message)) => format!("#message={message}"),
        Some(RefPart::Line(line)) => format!("#line={line}"),
    };
    format!("{base}{}{part}", typed.id)
}

/// A typed reference read back from its address, as [`address`] writes it or as an older export wrote it (without
/// a part, or with a query after it); none for any other destination. A part that is not one of the kind's own is
/// left out.
#[must_use]
pub fn typed(href: &str) -> Option<Typed> {
    let (head, fragment) = href.split_once('#').unwrap_or((href, ""));
    let path = head.split('?').next().unwrap_or(head).trim_end_matches('/');
    let mut file_path = None;
    let (kind, id) = if let Some(rest) = path.strip_prefix(ENVIRONMENTS) {
        let (id, path) = rest.split_once("/files/")?;
        let path = decode_path(path)?;
        if !relative_path(&path) {
            return None;
        }
        file_path = Some(path);
        (InlineRefKind::EnvironmentFile, id)
    } else if let Some(id) = path.strip_prefix(FILES) {
        (InlineRefKind::File, id)
    } else if let Some(id) = path.strip_prefix(NODES) {
        (InlineRefKind::Node, id)
    } else if let Some(id) = path.strip_prefix(DOCUMENTS) {
        (InlineRefKind::Document, id)
    } else {
        (InlineRefKind::Chat, path.strip_prefix(CHATS)?)
    };
    let id: Id = id.parse().ok()?;
    let part = match (kind, fragment.split_once('=')) {
        (InlineRefKind::Document, Some(("block", block))) => block.parse().ok().map(RefPart::Block),
        (InlineRefKind::Node, Some(("turn", turn))) => {
            turn.parse().ok().filter(|n| *n >= 1).map(RefPart::Turn)
        }
        (InlineRefKind::Chat, Some(("message", message))) => message
            .parse()
            .ok()
            .filter(|n| *n >= 1)
            .map(RefPart::Message),
        (InlineRefKind::File | InlineRefKind::EnvironmentFile, Some(("line", line))) => {
            line.parse().ok().filter(|n| *n >= 1).map(RefPart::Line)
        }
        _ => None,
    };
    Some(Typed {
        kind,
        id,
        path: file_path,
        part,
    })
}

/// Whether a stored workspace path names a file strictly inside the workspace.
#[must_use]
pub fn relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', ':'])
        && !path.chars().any(char::is_control)
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

/// A path written in a stored reference's address, preserving slashes and escaping URL punctuation.
#[must_use]
pub fn encode_path(path: &str) -> String {
    let mut out = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~/".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A stored address's path read back without accepting malformed escapes or invalid UTF-8.
#[must_use]
pub fn decode_path(path: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut at = 0;
    while at < path.len() {
        let byte = path.as_bytes()[at];
        if byte == b'%' {
            bytes.push(u8::from_str_radix(path.get(at + 1..at + 3)?, 16).ok()?);
            at += 3;
        } else {
            bytes.push(byte);
            at += 1;
        }
    }
    String::from_utf8(bytes).ok()
}

/// An agent's path, relative to its working directory or absolute inside its workspace, normalized to the root.
#[must_use]
pub fn workspace_path(path: &str, root: &str, directory: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let root = root.replace('\\', "/");
    let directory = directory.replace('\\', "/");
    let windows = root.as_bytes().get(1..3) == Some(b":/");
    let absolute = path.starts_with('/') || path.as_bytes().get(1..3) == Some(b":/");
    let path = if absolute {
        let prefix = format!("{}/", root.trim_end_matches('/'));
        let inside = if windows {
            path.get(..prefix.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(&prefix))
        } else {
            path.starts_with(&prefix)
        };
        if !inside {
            return None;
        }
        path[prefix.len()..].to_owned()
    } else if directory.is_empty() {
        path
    } else {
        format!("{directory}/{path}")
    };
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    let path = parts.join("/");
    relative_path(&path).then_some(path)
}

/// Whether a text in backquotes is meant as a path of the tree: one that names a thing by its identifier, ends in
/// `.md` (a document, a conversation, a chat) or in `/` (a place), or names a part. Other words in backquotes (a
/// command) are no reference. File-like words also allow a workspace lookup after the shared tree.
fn pathlike(text: &str) -> bool {
    let text = text.trim();
    text.starts_with(ID_PREFIX)
        || text.ends_with(".md")
        || text.ends_with('/')
        || text.contains(['/', '\\'])
        || (!text.contains(char::is_whitespace)
            && text
                .rsplit_once('.')
                .is_some_and(|(stem, ext)| !stem.is_empty() && !ext.is_empty()))
        || parse(text).is_some_and(|written| written.part.is_some())
}

/// Every link of `markdown` whose destination reads as a reference, and every text in backquotes outside a link that
/// reads as a path of the tree, in the order they stand. An image, an address written in angle brackets alone and
/// a mail address are none.
#[must_use]
pub fn find(markdown: &str) -> Vec<Found> {
    let mut out = Vec::new();
    // The links being read: where each began, its destination, and the words read so far.
    let mut open: Vec<(Range<usize>, Option<String>, String)> = Vec::new();
    let mut images = 0_usize;
    for (event, range) in Parser::new_ext(markdown, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Image { .. }) => images += 1,
            Event::End(TagEnd::Image) => images = images.saturating_sub(1),
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                ..
            }) => {
                let wanted =
                    !matches!(link_type, LinkType::Autolink | LinkType::Email) && images == 0;
                open.push((range, wanted.then(|| dest_url.to_string()), String::new()));
            }
            Event::End(TagEnd::Link) => {
                if let Some((span, Some(text), label)) = open.pop()
                    && parse(&text).is_some()
                {
                    out.push(Found {
                        span,
                        text,
                        label,
                        form: Form::Link,
                    });
                }
            }
            Event::Text(text) => {
                if let Some((_, _, label)) = open.last_mut() {
                    label.push_str(&text);
                }
            }
            Event::Code(code) => {
                if let Some((_, _, label)) = open.last_mut() {
                    label.push_str(&code);
                } else if images == 0 && pathlike(&code) && parse(&code).is_some() {
                    out.push(Found {
                        span: range,
                        text: code.to_string(),
                        label: code.to_string(),
                        form: Form::Code,
                    });
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|found| found.span.start);
    out
}

/// What was written as a link and is not one for Markdown, because its destination holds a space or a parenthesis
/// without angle brackets round it, as `[plan](docs/Rollout plan.md)`: such text stays words, so whoever wrote it
/// is told how a destination is written. Each as written, from `[` to its closing parenthesis, in the order they
/// stand; text in code and in links is not looked at.
#[must_use]
pub fn loose(markdown: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut words = String::new();
    // Inside a link, an image or a block of code, nothing is a loose link.
    let mut depth = 0_usize;
    for event in Parser::new_ext(markdown, options()) {
        match event {
            Event::Text(text) if depth == 0 => words.push_str(&text),
            Event::Start(Tag::Link { .. } | Tag::Image { .. } | Tag::CodeBlock(_)) => {
                loose_in(&words, &mut out);
                words.clear();
                depth += 1;
            }
            Event::End(TagEnd::Link | TagEnd::Image | TagEnd::CodeBlock) => {
                depth = depth.saturating_sub(1);
            }
            Event::Text(_) => {}
            _ => {
                loose_in(&words, &mut out);
                words.clear();
            }
        }
    }
    loose_in(&words, &mut out);
    out
}

/// The loose links of one run of words: `[`, words without brackets, `](`, then a destination up to the
/// parenthesis that closes it, which holds a space or a parenthesis of its own and does not begin with `<`.
fn loose_in(words: &str, out: &mut Vec<String>) {
    let mut from = 0;
    while let Some(at) = words[from..].find("](") {
        let middle = from + at;
        let start = words[..middle].rfind('[');
        let open = middle + 2;
        let mut depth = 1_usize;
        let mut end = None;
        for (offset, c) in words[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + offset);
                        break;
                    }
                }
                '\n' => break,
                _ => {}
            }
        }
        let (Some(start), Some(end)) = (start, end) else {
            from = open;
            continue;
        };
        let label = &words[start + 1..middle];
        let destination = &words[open..end];
        if !label.contains(['[', ']'])
            && !destination.trim().is_empty()
            && !destination.starts_with('<')
            && destination
                .trim()
                .chars()
                .any(|c| c.is_whitespace() || c == '(' || c == ')')
        {
            out.push(words[start..=end].to_owned());
        }
        from = end + 1;
    }
}

/// The typed references a text holds as links to their addresses, in the order they stand.
#[must_use]
pub fn typed_in(markdown: &str) -> Vec<Typed> {
    let mut out = Vec::new();
    for event in Parser::new_ext(markdown, options()) {
        if let Event::Start(Tag::Link { dest_url, .. }) = event
            && let Some(found) = typed(&dest_url)
        {
            out.push(found);
        }
    }
    out
}

/// What a link's destination or a text in backquotes came to, for the one reader of Markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reference {
    /// The typed reference it was resolved to: it becomes a chip.
    Typed(Typed),
    /// A link meant as a reference that did not resolve: its words are kept, without the link.
    Plain,
    /// Nothing to do with references: it stays as it was written.
    Untouched,
}

/// The destination of a link to `path` and its `part` as an agent is given it: as it is, or in angle brackets where
/// it holds a space, a parenthesis, an angle bracket or a backslash, which a destination in Markdown may not hold
/// bare; the brackets enclose the whole destination, the part with it.
#[must_use]
pub fn destination(path: &str, part: Option<i64>) -> String {
    let whole = match part {
        Some(number) => format!("{path}#{number}"),
        None => path.to_owned(),
    };
    let bare = !whole
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '(' | ')' | '<' | '>' | '\\'));
    if bare {
        return whole;
    }
    let mut out = String::with_capacity(whole.len() + 2);
    out.push('<');
    for c in whole.chars() {
        if matches!(c, '<' | '>' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('>');
    out
}

/// The words of a link, with what Markdown would read as markup in them escaped.
#[must_use]
pub fn label(words: &str) -> String {
    let mut out = String::with_capacity(words.len());
    for c in words.chars() {
        match c {
            '\\' | '[' | ']' | '`' | '*' | '_' | '<' | '>' | '$' | '~' | '&' | '|' => {
                out.push('\\');
                out.push(c);
            }
            '\n' | '\r' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

/// A link to a thing as an agent is given it: `[name](path#n)`, so that the path can be passed to `read_shared` as
/// it is and the number as `from`.
#[must_use]
pub fn agent_link(name: &str, path: &str, part: Option<i64>) -> String {
    format!("[{}]({})", label(name), destination(path, part))
}

/// The words that stand for the `number`th reference of a text that its reader may not read: they name nothing.
#[must_use]
pub fn hidden(number: usize) -> String {
    format!("({HIDDEN_WORDS} {number})")
}

/// The shared name of a reference its reader cannot access, with a kind-only legacy fallback.
#[must_use]
pub fn no_access(shared: Option<&str>, kind: InlineRefKind) -> String {
    let fallback = match kind {
        InlineRefKind::Node => "Place",
        InlineRefKind::Document => "Document",
        InlineRefKind::Chat => "Chat",
        InlineRefKind::File | InlineRefKind::EnvironmentFile => "File",
    };
    let name = shared
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(fallback);
    format!("{} (no access)", label(name))
}

/// Where [`hidden`] words stand in `text`, and the number of each.
#[must_use]
pub fn hidden_in(text: &str) -> Vec<(Range<usize>, usize)> {
    let opening = format!("({HIDDEN_WORDS} ");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find(&opening) {
        let start = from + at;
        let digits_start = start + opening.len();
        let digits = text[digits_start..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        let end = digits_start + digits;
        if digits > 0 && text[end..].starts_with(')') {
            if let Ok(number) = text[digits_start..end].parse() {
                out.push((start..end + 1, number));
            }
            from = end + 1;
        } else {
            from = digits_start;
        }
    }
    out
}

/// A text with every link to a reference's address in it written for one reader: as [`agent_link`] where `write`
/// gives a link, and as [`hidden`] words where it gives none. Hidden words are numbered on from `numbered`, which
/// comes back counted on, so that the texts of one answer number theirs as one.
#[must_use]
pub fn for_reader(
    markdown: &str,
    write: &dyn Fn(&Typed) -> Option<Writing>,
    numbered: &mut usize,
) -> String {
    let mut spans: Vec<(Range<usize>, String)> = Vec::new();
    let mut numbers: std::collections::HashMap<Typed, usize> = std::collections::HashMap::new();
    let mut depth = 0_usize;
    for (event, range) in Parser::new_ext(markdown, options()).into_offset_iter() {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                depth += 1;
                if depth == 1
                    && let Some(found) = typed(&dest_url)
                {
                    let text = match write(&found) {
                        Some(Writing::Link { name, path, part }) => agent_link(&name, &path, part),
                        Some(Writing::Hidden(_)) | None => {
                            let shared: String =
                                Parser::new_ext(&markdown[range.clone()], options())
                                    .filter_map(|event| match event {
                                        Event::Text(text) | Event::Code(text) => {
                                            Some(text.into_string())
                                        }
                                        _ => None,
                                    })
                                    .collect();
                            let words = no_access(Some(&shared), found.kind);
                            numbers.entry(found).or_insert_with(|| {
                                *numbered += 1;
                                *numbered
                            });
                            words
                        }
                    };
                    spans.push((range, text));
                }
            }
            Event::End(TagEnd::Link) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let mut out = markdown.to_owned();
    for (range, text) in spans.into_iter().rev() {
        out.replace_range(range, &text);
    }
    out
}

/// A text with the references that were resolved written as links to their addresses, for storing: each found one
/// for which `resolved` gives its typed reference becomes `[its words](address)`; the others stay as written.
#[must_use]
pub fn stored(markdown: &str, resolved: &dyn Fn(&Found) -> Option<Typed>) -> String {
    stored_with(markdown, &|found| match resolved(found) {
        Some(typed) => Reference::Typed(typed),
        None => Reference::Untouched,
    })
}

/// Stores resolved references by identity and failed reference links as their words alone.
#[must_use]
pub fn stored_with(markdown: &str, resolved: &dyn Fn(&Found) -> Reference) -> String {
    let mut out = markdown.to_owned();
    for found in find(markdown).into_iter().rev() {
        let words = if found.label.trim().is_empty() {
            &found.text
        } else {
            &found.label
        };
        let replacement = match resolved(&found) {
            Reference::Typed(typed) => format!("[{}]({})", label(words), address(&typed)),
            Reference::Plain => label(words),
            Reference::Untouched => continue,
        };
        out.replace_range(found.span, &replacement);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Id {
        Id::from_u128(n)
    }

    #[test]
    fn a_missing_senders_link_cannot_be_reinterpreted_in_a_receivers_namespace() {
        let text = "Read [these words](docs/Gone.md#1), `docs/Gone.md#1` and [the website](https://example.test).";
        let stored = stored_with(text, &|found| {
            if found.form == Form::Link && found.text == "docs/Gone.md#1" {
                Reference::Plain
            } else {
                Reference::Untouched
            }
        });
        assert_eq!(
            stored,
            "Read these words, `docs/Gone.md#1` and [the website](https://example.test)."
        );
    }

    #[test]
    fn a_path_or_an_identifier_reads_as_a_reference_with_the_part_its_number_names() {
        assert_eq!(
            parse("docs/Rollout plan.md#3"),
            Some(Written {
                path: "docs/Rollout plan.md".into(),
                part: Some(3)
            })
        );
        assert_eq!(
            parse(" /acme/web/Fix login/conversation.md "),
            Some(Written {
                path: "/acme/web/Fix login/conversation.md".into(),
                part: None
            })
        );
        assert_eq!(
            parse("id:0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01#12"),
            Some(Written {
                path: "id:0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01".into(),
                part: Some(12)
            })
        );
        assert_eq!(
            parse("docs/Issue #42.md"),
            Some(Written {
                path: "docs/Issue #42.md".into(),
                part: None
            }),
            "a # inside a name is no part"
        );
    }

    #[test]
    fn web_and_mail_addresses_anchors_and_typed_addresses_are_no_written_reference() {
        for text in [
            "https://example.com/docs/a.md",
            "HTTP://example.com",
            "mailto:dana@example.com",
            "#section",
            "//example.com/x",
            "",
            "   ",
            "/documents/0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01#block=0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_typed_reference_is_written_as_its_address_and_read_back_from_it() {
        let cases = [
            Typed {
                path: None,
                kind: InlineRefKind::Document,
                id: id(1),
                part: Some(RefPart::Block(id(2))),
            },
            Typed {
                path: None,
                kind: InlineRefKind::Node,
                id: id(3),
                part: Some(RefPart::Turn(4)),
            },
            Typed {
                path: None,
                kind: InlineRefKind::Chat,
                id: id(5),
                part: Some(RefPart::Message(120)),
            },
            Typed {
                path: None,
                kind: InlineRefKind::Node,
                id: id(6),
                part: None,
            },
        ];
        for case in cases {
            assert_eq!(typed(&address(&case)), Some(case.clone()), "{case:?}");
        }
        assert_eq!(
            typed(&format!("/documents/{}", id(7))),
            Some(Typed {
                path: None,
                kind: InlineRefKind::Document,
                id: id(7),
                part: None
            }),
            "an export's older form, without a part, is read as well"
        );
        assert_eq!(
            typed(&format!("/nodes/{}#block={}", id(8), id(9))).map(|typed| typed.part),
            Some(None),
            "a part that is not the kind's own is left out"
        );
        assert_eq!(typed("/elsewhere/x"), None);
    }

    #[test]
    fn links_and_backquoted_paths_are_found_where_markdown_reads_them() {
        let text = "See [the plan](<docs/Rollout plan.md#3>) and `Repro/conversation.md#4`, \
                    not [a site](https://example.com), not `make test` nor `src/main.rs`, \
                    and not ![an image](docs/a.png) nor <https://example.com/x>.\n\n```\n`docs/in code.md`\n```";
        let found = find(text);
        let summary: Vec<(&str, &str, Form)> = found
            .iter()
            .map(|one| (one.text.as_str(), one.label.as_str(), one.form))
            .collect();
        assert_eq!(
            summary,
            [
                ("docs/Rollout plan.md#3", "the plan", Form::Link),
                (
                    "Repro/conversation.md#4",
                    "Repro/conversation.md#4",
                    Form::Code
                ),
                ("src/main.rs", "src/main.rs", Form::Code),
            ],
            "words in backquotes, such as a command, are no reference"
        );
        assert_eq!(
            &text[found[0].span.clone()],
            "[the plan](<docs/Rollout plan.md#3>)"
        );
    }

    #[test]
    fn an_agent_is_given_a_link_whose_destination_markdown_reads_whole() {
        assert_eq!(
            agent_link("Rollout plan", "docs/Rollout plan.md", Some(3)),
            "[Rollout plan](<docs/Rollout plan.md#3>)"
        );
        assert_eq!(
            agent_link("Notes", "/acme/web/docs/Notes.md", None),
            "[Notes](/acme/web/docs/Notes.md)"
        );
        assert_eq!(
            agent_link("Plan (v2) [draft]", "docs/Plan (v2).md", Some(1)),
            "[Plan (v2) \\[draft\\]](<docs/Plan (v2).md#1>)"
        );
        assert_eq!(destination("a<b>\\c.md", None), "<a\\<b\\>\\\\c.md>");
        // What is given reads back as the same destination.
        for (path, part) in [
            ("docs/Rollout plan.md", Some(3)),
            ("Fix (login)/conversation.md", Some(12)),
            ("docs/a<b>\\c.md", None),
            ("/acme/web/docs/Notes.md", None),
        ] {
            let link = agent_link("x", path, part);
            let found = find(&link);
            assert_eq!(found.len(), 1, "{link}");
            assert_eq!(
                parse(&found[0].text),
                Some(Written {
                    path: path.into(),
                    part
                }),
                "{link}"
            );
        }
    }

    #[test]
    fn a_link_markdown_does_not_read_is_found_so_that_its_writer_is_told() {
        assert_eq!(
            loose(
                "See [plan](docs/Rollout plan.md#2), [ok](<docs/Rollout plan.md>), \
                 [v2](docs/Plan (v2).md) and [fine](docs/a.md)."
            ),
            ["[plan](docs/Rollout plan.md#2)", "[v2](docs/Plan (v2).md)"]
        );
        assert!(
            loose("```\n[x](a b)\n```\n\n`[y](c d)` and [z](e).").is_empty(),
            "code and real links are not looked at"
        );
    }

    #[test]
    fn file_references_keep_paths_and_lines_through_the_stored_form() {
        for typed in [
            Typed {
                kind: InlineRefKind::File,
                id: id(1),
                path: None,
                part: Some(RefPart::Line(7)),
            },
            Typed {
                kind: InlineRefKind::EnvironmentFile,
                id: id(2),
                path: Some("src/auth (new)#1.rs".into()),
                part: Some(RefPart::Line(42)),
            },
        ] {
            assert_eq!(self::typed(&address(&typed)), Some(typed));
        }
        assert_eq!(
            parse("src/auth.rs:42"),
            Some(Written {
                path: "src/auth.rs".into(),
                part: Some(42)
            })
        );
        assert_eq!(
            parse("src/auth#draft.rs:42"),
            Some(Written {
                path: "src/auth#draft.rs".into(),
                part: Some(42)
            })
        );
        assert_eq!(
            workspace_path("src/auth.rs", "/workspace", "web"),
            Some("web/src/auth.rs".into())
        );
        assert_eq!(
            workspace_path("/workspace/web/src/auth.rs", "/workspace", "web"),
            Some("web/src/auth.rs".into())
        );
        assert_eq!(
            workspace_path("../src/auth.rs", "/workspace", "web"),
            Some("src/auth.rs".into())
        );
        assert_eq!(
            workspace_path("../../etc/passwd", "/workspace", "web"),
            None
        );
        assert_eq!(
            workspace_path("/workspace-other/auth.rs", "/workspace", "web"),
            None
        );
        assert_eq!(
            workspace_path(r"C:\work\web\src\auth.rs", r"c:\work", "web"),
            Some("web/src/auth.rs".into())
        );
        assert_eq!(
            workspace_path(r"src\auth.rs", r"C:\work", r"web\api"),
            Some("web/api/src/auth.rs".into())
        );
        assert_eq!(
            workspace_path(r"C:\elsewhere\auth.rs", r"C:\work", "web"),
            None
        );
    }

    #[test]
    fn hidden_words_name_nothing_and_are_found_again_by_their_number() {
        assert_eq!(hidden(2), "(unavailable reference 2)");
        let text = "Before (unavailable reference 2), (unavailable reference x) and (unavailable reference 10).";
        let found = hidden_in(text);
        assert_eq!(
            found
                .iter()
                .map(|(range, number)| (&text[range.clone()], *number))
                .collect::<Vec<_>>(),
            [
                ("(unavailable reference 2)", 2),
                ("(unavailable reference 10)", 10)
            ]
        );
    }

    #[test]
    fn a_text_is_written_for_its_reader_with_links_and_hidden_words() {
        let open = Typed {
            path: None,
            kind: InlineRefKind::Document,
            id: id(1),
            part: Some(RefPart::Block(id(2))),
        };
        let closed = Typed {
            path: None,
            kind: InlineRefKind::Chat,
            id: id(3),
            part: None,
        };
        let text = format!(
            "Read [plan]({}) and [the chat]({}), and [a site](https://example.com).",
            address(&open),
            address(&closed)
        );
        let write = |typed: &Typed| {
            (typed == &open).then(|| Writing::Link {
                name: "Rollout plan".into(),
                path: "docs/Rollout plan.md".into(),
                part: Some(3),
            })
        };
        let mut numbered = 0;
        assert_eq!(
            for_reader(&text, &write, &mut numbered),
            "Read [Rollout plan](<docs/Rollout plan.md#3>) and the chat (no access), and [a site](https://example.com)."
        );
        assert_eq!(numbered, 1);
    }

    #[test]
    fn a_resolved_reference_is_stored_as_a_link_to_its_address_and_the_rest_stays_as_written() {
        let text = "Look at [the plan](docs/Plan.md#2), `Repro/` and [gone](docs/Gone.md).";
        let plan = Typed {
            path: None,
            kind: InlineRefKind::Document,
            id: id(1),
            part: Some(RefPart::Block(id(2))),
        };
        let repro = Typed {
            path: None,
            kind: InlineRefKind::Node,
            id: id(3),
            part: None,
        };
        let stored = stored(text, &|found| match found.text.as_str() {
            "docs/Plan.md#2" => Some(plan.clone()),
            "Repro/" => Some(repro.clone()),
            _ => None,
        });
        assert_eq!(
            stored,
            format!(
                "Look at [the plan]({}), [Repro/]({}) and [gone](docs/Gone.md).",
                address(&plan),
                address(&repro)
            )
        );
    }
}
