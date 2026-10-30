//! The line around a mention of a thing, as one reader reads it: the thing under its name now, other chips named only where the reader may know them.
//!
//! A source says where it mentions a document in three forms: inline content (a block, a comment), Markdown text
//! (a chat message, what a person or an agent said in a thread) and a link mark on words. Each is flattened to plain
//! words, the first mention is marked, and a window of about the asked number of characters is cut round it.
//!
//! Nothing here reads the database or decides who may read what: the caller says what each other chip is called for
//! this reader, and this module only chooses words.

use crate::references::{self, Typed};
use orch_api_contract::documents::BlockContent;
use orch_api_contract::documents::{Inline, InlineRefKind};
use orch_types::{Id, Json};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// How far back from a cut a word may be given up to keep it whole: a longer word is cut where the limit falls.
const WORD_SLACK_CHARS: usize = 12;
/// What marks text that was cut away.
const ELLIPSIS: &str = "…";

/// What stands for each chip other than the target, for one reader.
pub trait Names {
    /// The name the reader may know a thing by now; none where they may not read it.
    fn chip(&self, kind: InlineRefKind, id: Id, path: Option<&str>) -> Option<String>;
    /// The name of a person.
    fn person(&self, id: Id) -> Option<String>;
    /// What stands for a chip the reader may not know: by default the words its author wrote for it, which a person
    /// reads in the source already.
    fn hidden(&self, kind: InlineRefKind, label: Option<&str>) -> String {
        match label.map(str::trim).filter(|label| !label.is_empty()) {
            Some(label) => label.to_owned(),
            None => references::no_access(None, kind),
        }
    }
}

/// The thing a line is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// What kind of thing.
    pub kind: InlineRefKind,
    /// Which one.
    pub id: Id,
}

/// The words of one mention in order: before it, the reference under its name now, and after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The words before the reference, beginning with an ellipsis where text was cut.
    pub before: String,
    /// The reference itself.
    pub reference: String,
    /// The words after it, ending with an ellipsis where text was cut.
    pub after: String,
}

/// Plain words of a source with the first mention marked: what stands before it, and after it.
struct Flat {
    before: String,
    after: String,
    found: bool,
}

impl Flat {
    fn new() -> Self {
        Self {
            before: String::new(),
            after: String::new(),
            found: false,
        }
    }

    fn plain(&mut self, words: &str) {
        if self.found {
            self.after.push_str(words);
        } else {
            self.before.push_str(words);
        }
    }

    /// The target stands here; the first one is the mention, later ones read as plain words.
    fn target(&mut self, name: &str) {
        if self.found {
            self.after.push_str(name);
        } else {
            self.found = true;
        }
    }
}

fn is_target(typed: &Typed, target: Target) -> bool {
    typed.kind == target.kind && typed.id == target.id
}

fn flatten_runs(runs: &[Inline], target: Target, names: &dyn Names, flat: &mut Flat, name: &str) {
    for run in runs {
        match run {
            Inline::Text { text, href, .. } => {
                let linked = href
                    .as_deref()
                    .and_then(references::typed)
                    .is_some_and(|typed| is_target(&typed, target));
                if linked {
                    flat.target(name);
                } else {
                    flat.plain(text);
                }
            }
            Inline::Mention { user_id, .. } => {
                if let Some(person) = names.person(*user_id) {
                    flat.plain(&format!("@{person}"));
                }
            }
            Inline::Ref {
                kind,
                id,
                path,
                label,
                ..
            } => {
                if *kind == target.kind && *id == target.id {
                    flat.target(name);
                } else {
                    let known = names.chip(*kind, *id, path.as_deref());
                    flat.plain(&known.unwrap_or_else(|| names.hidden(*kind, label.as_deref())));
                }
            }
            Inline::Math { source } => flat.plain(source),
            Inline::Break => flat.plain(" "),
        }
    }
}

/// The line around the first mention of `target` in inline content; none where it is not mentioned.
#[must_use]
pub fn line_in_runs(
    runs: &[Inline],
    target: Target,
    name: &str,
    names: &dyn Names,
    chars: usize,
) -> Option<Line> {
    let mut flat = Flat::new();
    flatten_runs(runs, target, names, &mut flat, name);
    flat.found
        .then(|| window(&flat.before, name, &flat.after, chars))
}

/// The line around the first mention of `target` in a block's content; the cells of a table are read row by row.
#[must_use]
pub fn line_in_content(
    content: &BlockContent,
    target: Target,
    name: &str,
    names: &dyn Names,
    chars: usize,
) -> Option<Line> {
    match content {
        BlockContent::Inline(runs) => line_in_runs(runs, target, name, names, chars),
        BlockContent::Rows(rows) => {
            let mut flat = Flat::new();
            for (at, cell) in rows.iter().flat_map(|row| row.cells.iter()).enumerate() {
                if at > 0 {
                    flat.plain(" | ");
                }
                flatten_runs(&cell.content, target, names, &mut flat, name);
            }
            flat.found
                .then(|| window(&flat.before, name, &flat.after, chars))
        }
    }
}

/// The line around the first link to `target` in Markdown text: the words of the text without its marks, the target
/// under its name, other links to things as the reader may name them and otherwise as their author labelled them.
#[must_use]
pub fn line_in_markdown(
    text: &str,
    target: Target,
    name: &str,
    names: &dyn Names,
    chars: usize,
) -> Option<Line> {
    if !references::typed_in(text)
        .iter()
        .any(|typed| is_target(typed, target))
    {
        return None;
    }
    let mut flat = Flat::new();
    let mut link: Option<(Option<Typed>, String)> = None;
    for event in Parser::new_ext(text, references::options()) {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                link = Some((references::typed(&dest_url), String::new()));
            }
            Event::End(TagEnd::Link) => {
                if let Some((typed, label)) = link.take() {
                    match typed {
                        Some(typed) if is_target(&typed, target) => flat.target(name),
                        Some(typed) => flat.plain(
                            &names
                                .chip(typed.kind, typed.id, typed.path.as_deref())
                                .unwrap_or_else(|| names.hidden(typed.kind, Some(label.trim()))),
                        ),
                        None => flat.plain(&label),
                    }
                }
            }
            Event::Text(words) | Event::Code(words) => match &mut link {
                Some((_, label)) => label.push_str(&words),
                None => flat.plain(&words),
            },
            // Block ends and breaks are a space; the ends of marks (strong, emphasis) are none.
            Event::SoftBreak
            | Event::HardBreak
            | Event::Rule
            | Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::Item
                | TagEnd::TableCell
                | TagEnd::TableRow
                | TagEnd::BlockQuote(_)
                | TagEnd::CodeBlock,
            ) => match &mut link {
                Some((_, label)) => label.push(' '),
                None => flat.plain(" "),
            },
            _ => {}
        }
    }
    flat.found
        .then(|| window(&flat.before, name, &flat.after, chars))
}

/// The line around the first mention of `target` in any text a stored value holds: a chat message's parts, what an
/// item of a thread says.
#[must_use]
pub fn line_in_value(
    value: &Json,
    target: Target,
    name: &str,
    names: &dyn Names,
    chars: usize,
) -> Option<Line> {
    match value {
        Json::String(text) => line_in_markdown(text, target, name, names, chars),
        Json::Array(items) => items
            .iter()
            .find_map(|item| line_in_value(item, target, name, names, chars)),
        Json::Object(members) => members
            .values()
            .find_map(|item| line_in_value(item, target, name, names, chars)),
        _ => None,
    }
}

/// Every typed reference the text of a stored value holds as a link to a thing's address, in the order they stand.
#[must_use]
pub fn typed_in_value(value: &Json) -> Vec<Typed> {
    match value {
        Json::String(text) => references::typed_in(text),
        Json::Array(items) => items.iter().flat_map(typed_in_value).collect(),
        Json::Object(members) => members.values().flat_map(typed_in_value).collect(),
        _ => Vec::new(),
    }
}

/// The first words of a stored value, for a part that names its target in no words (a quoted item): no reference.
#[must_use]
pub fn opening_of_value(value: &Json, chars: usize) -> Option<Line> {
    fn first_text(value: &Json) -> Option<&str> {
        match value {
            Json::String(text) if !text.trim().is_empty() => Some(text),
            Json::Array(items) => items.iter().find_map(first_text),
            Json::Object(members) => members.values().find_map(first_text),
            _ => None,
        }
    }
    let text = first_text(value)?;
    Some(window("", "", &collapse(text), chars))
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_wide_script(character: char) -> bool {
    matches!(character as u32,
        0x2E80..=0x9FFF | 0xA000..=0xA4CF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF)
}

/// The last `keep` characters of `words`, from a word's beginning where that costs little; cut where one begins.
fn tail(words: &str, keep: usize) -> (String, bool) {
    let all: Vec<char> = words.chars().collect();
    if all.len() <= keep {
        return (words.to_owned(), false);
    }
    if keep == 0 {
        return (String::new(), true);
    }
    let mut start = all.len() - keep;
    let splits_word = !all[start - 1].is_whitespace()
        && !all[start].is_whitespace()
        && !is_wide_script(all[start])
        && !is_wide_script(all[start - 1]);
    if splits_word
        && let Some(gap) = all[start..]
            .iter()
            .take(WORD_SLACK_CHARS)
            .position(|character| character.is_whitespace())
    {
        start += gap;
    }
    let cut: String = all[start..].iter().collect();
    (cut.trim_start().to_owned(), true)
}

/// The first `keep` characters of `words`, to a word's end where that costs little.
fn head(words: &str, keep: usize) -> (String, bool) {
    let all: Vec<char> = words.chars().collect();
    if all.len() <= keep {
        return (words.to_owned(), false);
    }
    if keep == 0 {
        return (String::new(), true);
    }
    let mut end = keep;
    let splits_word = !all[end - 1].is_whitespace()
        && !all[end].is_whitespace()
        && !is_wide_script(all[end])
        && !is_wide_script(all[end - 1]);
    if splits_word
        && let Some(gap) = all[..end]
            .iter()
            .rev()
            .take(WORD_SLACK_CHARS)
            .position(|character| character.is_whitespace())
    {
        end -= gap;
    }
    let cut: String = all[..end].iter().collect();
    (cut.trim_end().to_owned(), true)
}

/// Cuts about `chars` characters round the reference: the reference whole, then the rest shared between the two sides,
/// a side that is short giving what it does not use to the other.
#[must_use]
pub fn window(before: &str, reference: &str, after: &str, chars: usize) -> Line {
    let before = collapse(before);
    let after = collapse(after);
    let reference = collapse(reference);
    let reference_len = reference.chars().count();
    let (reference, reference_len) = if reference_len > chars {
        let (shown, _) = head(&reference, chars.saturating_sub(1).max(1));
        let len = shown.chars().count() + 1;
        (format!("{shown}{ELLIPSIS}"), len)
    } else {
        (reference, reference_len)
    };
    let rest = chars.saturating_sub(reference_len);
    let half = rest / 2;
    let before_len = before.chars().count();
    let after_len = after.chars().count();
    let (before_room, after_room) = if before_len < half {
        (before_len, rest - before_len)
    } else if after_len < rest - half {
        (rest - after_len, after_len)
    } else {
        (half, rest - half)
    };
    let (before_cut, before_more) = tail(&before, before_room);
    let (after_cut, after_more) = head(&after, after_room);
    Line {
        before: if before_more {
            format!("{ELLIPSIS} {before_cut} ")
        } else if before_cut.is_empty() {
            String::new()
        } else {
            format!("{before_cut} ")
        },
        reference,
        after: if after_more {
            format!(" {after_cut} {ELLIPSIS}")
        } else if after_cut.is_empty() {
            String::new()
        } else {
            format!(" {after_cut}")
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::documents::{Inline, InlineRefKind};
    use std::collections::HashMap;

    struct Known(HashMap<Id, String>);

    impl Names for Known {
        fn chip(&self, _kind: InlineRefKind, id: Id, _path: Option<&str>) -> Option<String> {
            self.0.get(&id).cloned()
        }
        fn person(&self, id: Id) -> Option<String> {
            self.0.get(&id).cloned()
        }
    }

    fn id(n: u128) -> Id {
        Id::from_u128(n)
    }

    fn text(words: &str) -> Inline {
        Inline::Text {
            text: words.into(),
            marks: None,
            href: None,
            color: None,
            background: None,
        }
    }

    fn chip(of: Id, label: &str) -> Inline {
        Inline::Ref {
            display: None,
            kind: InlineRefKind::Document,
            id: of,
            path: None,
            part: None,
            label: Some(label.into()),
            name: None,
        }
    }

    const TARGET: Target = Target {
        kind: InlineRefKind::Document,
        id: Id::from_u128(1),
    };

    #[test]
    fn the_target_is_named_as_it_is_now_and_the_words_around_it_stay() {
        let runs = [
            text("See "),
            chip(id(1), "Old title"),
            text(" for the dates."),
        ];
        let line =
            line_in_runs(&runs, TARGET, "Rollout plan", &Known(HashMap::new()), 140).unwrap();
        assert_eq!(line.before, "See ");
        assert_eq!(line.reference, "Rollout plan");
        assert_eq!(line.after, " for the dates.");
    }

    #[test]
    fn another_chip_is_named_only_where_the_reader_may_know_it_and_otherwise_as_written() {
        let runs = [
            chip(id(2), "Budget"),
            text(" and "),
            chip(id(3), "Secret plan"),
            text(" lead to "),
            chip(id(1), "Plan"),
        ];
        let names = Known(HashMap::from([(id(2), "Budget 2027".to_owned())]));
        let line = line_in_runs(&runs, TARGET, "Plan", &names, 140).unwrap();
        assert_eq!(line.before, "Budget 2027 and Secret plan lead to ");
        // Another chip's written label is what the reader reads in the source already.
        let none = line_in_runs(
            &[chip(id(3), "Secret plan"), chip(id(1), "x")],
            TARGET,
            "Plan",
            &Known(HashMap::new()),
            140,
        )
        .unwrap();
        assert_eq!(none.before, "Secret plan ");
    }

    #[test]
    fn a_part_that_does_not_mention_the_target_has_no_line() {
        assert!(
            line_in_runs(
                &[text("nothing"), chip(id(2), "x")],
                TARGET,
                "Plan",
                &Known(HashMap::new()),
                140
            )
            .is_none()
        );
    }

    #[test]
    fn a_long_text_is_cut_round_the_reference_with_ellipses_at_word_edges() {
        let before = "alpha ".repeat(40);
        let after = " omega".repeat(40);
        let runs = [text(&before), chip(id(1), "x"), text(&after)];
        let line = line_in_runs(&runs, TARGET, "Plan", &Known(HashMap::new()), 60).unwrap();
        assert!(line.before.starts_with("… "), "{line:?}");
        assert!(line.after.ends_with(" …"), "{line:?}");
        let total = line.before.chars().count()
            + line.reference.chars().count()
            + line.after.chars().count();
        assert!(total <= 60 + 6, "{total} {line:?}");
        assert!(!line.before.contains("alph "), "no word is split: {line:?}");
    }

    #[test]
    fn a_short_side_gives_its_room_to_the_other() {
        let runs = [text("Go: "), chip(id(1), "x"), text(&" tail".repeat(50))];
        let line = line_in_runs(&runs, TARGET, "Plan", &Known(HashMap::new()), 80).unwrap();
        assert_eq!(line.before, "Go: ");
        assert!(line.after.chars().count() > 50, "{line:?}");
    }

    #[test]
    fn a_link_mark_on_words_is_the_mention_under_the_current_name() {
        let link = Inline::Text {
            text: "the old words".into(),
            marks: None,
            href: Some(format!("/documents/{}", id(1))),
            color: None,
            background: None,
        };
        let line = line_in_runs(
            &[text("Read "), link],
            TARGET,
            "Plan",
            &Known(HashMap::new()),
            140,
        )
        .unwrap();
        assert_eq!(line.reference, "Plan");
        assert_eq!(line.before, "Read ");
    }

    #[test]
    fn markdown_loses_its_marks_and_names_links_to_things() {
        let text = format!(
            "**Decided**: use [Budget](/documents/{}) with [the old name](/documents/{}) and [site](https://example.com).",
            id(2),
            id(1)
        );
        let names = Known(HashMap::from([(id(2), "Budget 2027".to_owned())]));
        let line = line_in_markdown(&text, TARGET, "Plan", &names, 140).unwrap();
        assert_eq!(line.before, "Decided: use Budget 2027 with ");
        assert_eq!(line.reference, "Plan");
        assert_eq!(line.after, " and site.");
        assert!(line_in_markdown("no links here", TARGET, "Plan", &names, 140).is_none());
    }

    #[test]
    fn a_stored_value_is_searched_for_the_first_text_that_mentions_the_target() {
        let value = serde_json::json!([
            {"type": "text", "text": "first, nothing"},
            {"type": "text", "text": format!("second [x](/documents/{})", id(1))}
        ]);
        let line = line_in_value(&value, TARGET, "Plan", &Known(HashMap::new()), 140).unwrap();
        assert_eq!(line.before, "second ");
    }

    #[test]
    fn wide_script_is_cut_where_the_limit_falls() {
        let before = "字".repeat(100);
        let line = window(&before, "目标", "", 20);
        assert!(line.before.starts_with("… "));
        assert_eq!(line.before.chars().count(), 2 + 18 + 1);
    }

    #[test]
    fn a_reference_longer_than_the_window_is_cut_too() {
        let line = window("a", &"n".repeat(300), "b", 40);
        assert!(line.reference.ends_with('…'));
        assert!(line.reference.chars().count() <= 40);
    }
}
