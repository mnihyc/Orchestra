//! The closed set of tags that Orch's Markdown is written with beside GitHub's, found line by line before the text is read as Markdown.
//!
//! A tag of the set that stands alone on a line (`<details>`, `<summary>…</summary>`, `<columns>`, `<column ratio="2">`,
//! `<page-break/>`, `<video src="…">caption</video>` and their closing tags) is what [`scan`] finds, outside fenced
//! code, whatever the line is indented by and whatever quote marks lead it. Only tags that open and close in order are
//! kept as tags: a tag without its partner stays in the text as the words it was written with, like any other tag
//! that is not in the set. A body that its writer indented under its opening tag, as Notion's Markdown does, is
//! taken to its tag's level. Each kept tag is put in the text as a line of its own between two blank lines, as a
//! mark that no Markdown can mean anything by, so that the reader of Markdown finds it as a paragraph wherever the
//! text has put it: in a list item, in a quote, at the top.

use std::collections::HashSet;
use std::fmt::Write as _;

/// How many containers may stand inside one another: a document holds sixteen levels of blocks, and a toggle or a column
/// is one of them.
// Each of sixteen document levels may add an annotation and two native container layers.
const TAG_DEPTH_MAX: usize = 48;

/// What a container tag opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// A toggle: `<details>` or `<toggle>`.
    Toggle,
    /// Identity-free block layout comments.
    Layout,
    /// Columns side by side.
    Columns,
    /// One of the columns.
    Column,
}

/// A tag of the set, as it stands on a line.
#[derive(Debug, Clone, PartialEq)]
pub enum BlockTag {
    /// A bounded JSON layout annotation beside ordinary Markdown.
    Layout(String),
    /// Separates a block from children that Markdown would otherwise flatten.
    LayoutChildren,
    /// The opening of a toggle; the words of its `<summary>` where the tag came with one.
    Toggle {
        /// The title, as Markdown.
        summary: Option<String>,
    },
    /// The opening of columns.
    Columns,
    /// The opening of one column, with the weight it asks for.
    Column {
        /// Its `ratio`.
        share: Option<f64>,
    },
    /// The closing of a container.
    Close(Container),
    /// A page break.
    PageBreak,
    /// A film or a sound.
    Media {
        /// Whether it is a sound.
        audio: bool,
        /// Where it is: a path or an address.
        src: String,
        /// Its caption, as Markdown.
        caption: String,
    },
}

/// The text of a document read for its tags: the text with each tag a mark, and the tags in the order of their marks.
#[derive(Debug, Clone, PartialEq)]
pub struct Scanned {
    /// The text with a line for each tag in place of the line it stood on.
    pub text: String,
    /// The tags, by the number their mark has; each with the line it was written as.
    pub tags: Vec<(BlockTag, String)>,
    /// The character a mark is made of.
    pub mark: char,
    /// The names of tags of the set that have no partner, and so stay in the text as the words they were written with
    /// (`/columns` for a closing tag); each once.
    pub unmatched: Vec<String>,
}

impl Scanned {
    /// The tag a text stands for, if it is exactly one mark.
    #[must_use]
    pub fn mark_at(&self, text: &str) -> Option<usize> {
        let inner = text
            .trim()
            .strip_prefix(self.mark)?
            .strip_suffix(self.mark)?;
        inner
            .parse::<usize>()
            .ok()
            .filter(|index| *index < self.tags.len())
    }

    /// A text with every mark in it put back as the line it stood for.
    #[must_use]
    pub fn restored(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find(self.mark) {
            out.push_str(&rest[..start]);
            let after = &rest[start + self.mark.len_utf8()..];
            let Some(end) = after.find(self.mark) else {
                out.push_str(&rest[start..]);
                return out;
            };
            match after[..end]
                .parse::<usize>()
                .ok()
                .and_then(|index| self.tags.get(index))
            {
                Some((_, written)) => out.push_str(written.trim()),
                None => out.push_str(
                    &rest[start..start + self.mark.len_utf8() + end + self.mark.len_utf8()],
                ),
            }
            rest = &after[end + self.mark.len_utf8()..];
        }
        out.push_str(rest);
        out
    }
}

/// One tag read from the start of a text.
#[derive(Debug, Clone, PartialEq)]
pub struct Tag<'t> {
    /// Its name, in lower case.
    pub name: String,
    /// Whether it is a closing tag.
    pub closing: bool,
    /// Whether it closes itself (`<br/>`).
    pub self_closing: bool,
    /// Its attributes, names in lower case; a word without a value has an empty one.
    pub attrs: Vec<(String, String)>,
    /// What follows it.
    pub rest: &'t str,
}

impl Tag<'_> {
    /// The value of an attribute.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(each, _)| each == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Reads one tag from the start of `text`: `<name attr="value" …>`, `</name>` or `<name … />`.
#[must_use]
pub fn parse_tag(text: &str) -> Option<Tag<'_>> {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    let mut at = 1;
    let closing = bytes.get(at) == Some(&b'/');
    if closing {
        at += 1;
    }
    let start = at;
    while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'-') {
        at += 1;
    }
    if at == start || !bytes[start].is_ascii_alphabetic() {
        return None;
    }
    let name = text[start..at].to_ascii_lowercase();
    let mut attrs: Vec<(String, String)> = Vec::new();
    loop {
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        match bytes.get(at)? {
            b'>' => {
                return Some(Tag {
                    name,
                    closing,
                    self_closing: false,
                    attrs,
                    rest: &text[at + 1..],
                });
            }
            b'/' => {
                if bytes.get(at + 1) == Some(&b'>') {
                    return Some(Tag {
                        name,
                        closing,
                        self_closing: true,
                        attrs,
                        rest: &text[at + 2..],
                    });
                }
                return None;
            }
            _ => {}
        }
        let from = at;
        while at < bytes.len()
            && !bytes[at].is_ascii_whitespace()
            && !matches!(bytes[at], b'=' | b'>' | b'/')
        {
            at += 1;
        }
        if at == from {
            return None;
        }
        let attribute = text[from..at].to_ascii_lowercase();
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let mut value = String::new();
        if bytes.get(at) == Some(&b'=') {
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            match bytes.get(at)? {
                quote @ (b'"' | b'\'') => {
                    let close = text[at + 1..].find(char::from(*quote))?;
                    value = text[at + 1..at + 1 + close].to_owned();
                    at += close + 2;
                }
                _ => {
                    let from = at;
                    while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'>'
                    {
                        at += 1;
                    }
                    value = text[from..at].to_owned();
                }
            }
        }
        attrs.push((attribute, value));
    }
}

/// The words between `<summary>` and `</summary>` of a text that is exactly that.
fn summary_of(text: &str) -> Option<String> {
    let tag = parse_tag(text.trim())?;
    if tag.name != "summary" || tag.closing || tag.self_closing {
        return None;
    }
    let rest = tag.rest.trim_end();
    let cut = rest.len().checked_sub("</summary>".len())?;
    if !rest.is_char_boundary(cut) || !rest[cut..].eq_ignore_ascii_case("</summary>") {
        return None;
    }
    Some(rest[..cut].trim().to_owned())
}

/// The weight a column asks for: a number, or a percentage.
fn weight_of(value: &str) -> Option<f64> {
    let value = value.trim();
    let (number, divisor) = match value.strip_suffix('%') {
        Some(number) => (number, 100.0),
        None => (value, 1.0),
    };
    number
        .trim()
        .parse::<f64>()
        .ok()
        .map(|number| number / divisor)
        .filter(|weight| weight.is_finite() && *weight > 0.0)
}

enum Line {
    Tag(BlockTag),
    Summary(String),
}

/// What a line is, if it is exactly one tag of the set.
fn classify(core: &str) -> Option<Line> {
    if core == "<!-- /orch:block -->" {
        return Some(Line::Tag(BlockTag::Close(Container::Layout)));
    }
    if core == "<!-- orch:children -->" {
        return Some(Line::Tag(BlockTag::LayoutChildren));
    }
    if let Some(metadata) = core
        .strip_prefix("<!-- orch:block ")
        .and_then(|value| value.strip_suffix(" -->"))
        .filter(|value| value.len() <= 4 * 64 * 1024)
    {
        return Some(Line::Tag(BlockTag::Layout(metadata.to_owned())));
    }
    let tag = parse_tag(core)?;
    let rest = tag.rest.trim();
    let alone = rest.is_empty();
    let found = match (tag.name.as_str(), tag.closing) {
        ("page-break", false) if alone => BlockTag::PageBreak,
        ("toggle" | "details", false) if !tag.self_closing => {
            if alone {
                BlockTag::Toggle { summary: None }
            } else {
                BlockTag::Toggle {
                    summary: Some(summary_of(rest)?),
                }
            }
        }
        ("toggle" | "details", true) if alone => BlockTag::Close(Container::Toggle),
        ("summary", false) => return summary_of(core).map(Line::Summary),
        ("columns", false) if alone && !tag.self_closing => BlockTag::Columns,
        ("columns", true) if alone => BlockTag::Close(Container::Columns),
        ("column", false) if alone && !tag.self_closing => BlockTag::Column {
            share: ["ratio", "share", "width"]
                .iter()
                .find_map(|name| tag.attr(name))
                .and_then(weight_of),
        },
        ("column", true) if alone => BlockTag::Close(Container::Column),
        (kind @ ("video" | "audio"), false) => {
            let src = tag.attr("src").filter(|src| !src.trim().is_empty())?;
            let caption = if tag.self_closing && alone {
                String::new()
            } else {
                let closing = format!("</{kind}>");
                let cut = rest.len().checked_sub(closing.len())?;
                if !rest.is_char_boundary(cut) || !rest[cut..].eq_ignore_ascii_case(&closing) {
                    return None;
                }
                rest[..cut].trim().to_owned()
            };
            BlockTag::Media {
                audio: kind == "audio",
                src: src.trim().to_owned(),
                caption,
            }
        }
        _ => return None,
    };
    Some(Line::Tag(found))
}

fn container_of(tag: &BlockTag) -> Option<Container> {
    match tag {
        BlockTag::Toggle { .. } => Some(Container::Toggle),
        BlockTag::Layout(_) => Some(Container::Layout),
        BlockTag::Columns => Some(Container::Columns),
        BlockTag::Column { .. } => Some(Container::Column),
        _ => None,
    }
}

/// What leads a line before its text: the white space and the marks of a quote.
fn leading(line: &str) -> &str {
    let end = line
        .find(|c: char| !matches!(c, ' ' | '\t' | '>'))
        .unwrap_or(line.len());
    &line[..end]
}

fn white_space(line: &str) -> &str {
    let end = line
        .find(|c: char| !matches!(c, ' ' | '\t'))
        .unwrap_or(line.len());
    &line[..end]
}

/// The text of a line without what lead it, and without the marker of a list item before a fence.
fn fence_core(line: &str) -> &str {
    let mut core = line.trim_start_matches([' ', '\t', '>']);
    // A list item may open a fence on its own line: `- ```rust`.
    if let Some(after) = core
        .strip_prefix(['-', '*', '+'])
        .filter(|after| after.starts_with([' ', '\t']))
    {
        core = after.trim_start();
    } else {
        let digits = core.chars().take_while(char::is_ascii_digit).count();
        if digits > 0
            && let Some(after) = core[digits..].strip_prefix(['.', ')'])
            && after.starts_with([' ', '\t'])
        {
            core = after.trim_start();
        }
    }
    core
}

/// How a line opens a fenced block of code: the character and the length of its fence.
fn fence_of(line: &str) -> Option<(char, usize)> {
    let core = fence_core(line);
    let fence = core.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let length = core.chars().take_while(|c| *c == fence).count();
    (length >= 3).then_some((fence, length))
}

/// Whether a line closes the fence a block of code was opened with.
fn closes_fence(line: &str, fence: char, length: usize) -> bool {
    let core = fence_core(line).trim_end();
    core.chars().count() >= length && core.chars().all(|c| c == fence)
}

/// A character that nothing in the text uses, for the mark of a tag.
fn free_mark(text: &str) -> Option<char> {
    ('\u{e000}'..='\u{f8ff}').find(|mark| !text.contains(*mark))
}

/// Finds the tags of the set in a text and marks each: `None` where the text has none to keep.
#[must_use]
pub fn scan(text: &str) -> Option<Scanned> {
    let lower = text.to_ascii_lowercase();
    if ![
        "<!-- orch:",
        "<!-- /orch:",
        "<toggle",
        "<details",
        "<columns",
        "<column",
        "<page-break",
        "<video",
        "<audio",
    ]
    .iter()
    .any(|name| lower.contains(name))
    {
        return None;
    }
    let mark = free_mark(text)?;
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    // The lines that are tags, outside the fenced code.
    let mut found: Vec<(usize, Line)> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for (index, line) in lines.iter().enumerate() {
        match fence {
            Some((character, length)) => {
                if closes_fence(line, character, length) {
                    fence = None;
                }
                continue;
            }
            None => {
                if let Some(opened) = fence_of(line) {
                    fence = Some(opened);
                    continue;
                }
            }
        }
        let core = line.trim_start_matches([' ', '\t', '>']).trim_end();
        if core.starts_with('<')
            && let Some(read) = classify(core)
        {
            names.push(parse_tag(core).map_or_else(
                || "orch:block".to_owned(),
                |tag| {
                    if tag.closing {
                        format!("/{}", tag.name)
                    } else {
                        tag.name
                    }
                },
            ));
            found.push((index, read));
        }
    }
    if found.is_empty() {
        return None;
    }
    // Which of them open and close in order. The opens not yet closed are a stack of all kinds, for the depth and for
    // dropping what was opened inside a container that closes; and the last of each kind is kept apart, so that the
    // partner of a close is found without looking through the others: the pairing is linear in the lines.
    let mut keep = vec![false; found.len()];
    let mut partner: Vec<Option<usize>> = vec![None; found.len()];
    let mut depth_of = vec![0_usize; found.len()];
    let mut open: Vec<usize> = Vec::new();
    let mut last: [Vec<usize>; 4] = std::array::from_fn(|_| Vec::new());
    let slot = |kind: Container| match kind {
        Container::Toggle => 0,
        Container::Layout => 3,
        Container::Columns => 1,
        Container::Column => 2,
    };
    for (at, (_, read)) in found.iter().enumerate() {
        let Line::Tag(tag) = read else { continue };
        if let Some(kind) = container_of(tag) {
            depth_of[at] = open.len();
            open.push(at);
            last[slot(kind)].push(at);
        } else if let BlockTag::Close(kind) = tag {
            let Some(&opened) = last[slot(*kind)].last() else {
                continue;
            };
            // Containers nested deeper than a document holds are words, the opening and the closing alike.
            if depth_of[opened] < TAG_DEPTH_MAX {
                keep[opened] = true;
                keep[at] = true;
                partner[opened] = Some(at);
            }
            // What was opened inside and never closed stays words.
            while let Some(top) = open.pop() {
                if let Line::Tag(each) = &found[top].1
                    && let Some(each) = container_of(each)
                {
                    last[slot(each)].pop();
                }
                if top == opened {
                    break;
                }
            }
        } else {
            keep[at] = true;
        }
    }
    // A summary that stands right after the opening of a toggle is its title.
    let mut gone: HashSet<usize> = HashSet::new();
    let mut titles: Vec<(usize, String)> = Vec::new();
    for at in 1..found.len() {
        let (line, Line::Summary(title)) = (&found[at].0, &found[at].1) else {
            continue;
        };
        let (before, Line::Tag(BlockTag::Toggle { summary: None })) =
            (&found[at - 1].0, &found[at - 1].1)
        else {
            continue;
        };
        if keep[at - 1]
            && lines[before + 1..*line]
                .iter()
                .all(|between| between.trim().is_empty())
        {
            titles.push((at - 1, title.clone()));
            gone.insert(*line);
        }
    }
    for (at, title) in titles {
        if let Line::Tag(BlockTag::Toggle { summary }) = &mut found[at].1 {
            *summary = Some(title);
        }
    }
    // A body its writer indented under the tag is taken to the level of the tag.
    let mut order: Vec<usize> = (0..found.len())
        .filter(|at| keep[*at] && partner[*at].is_some())
        .collect();
    order.sort_by_key(|at| found[*at].0);
    for opened in order {
        let (Some(closed), first) = (partner[opened], found[opened].0) else {
            continue;
        };
        let last = found[closed].0;
        let own = white_space(&lines[first]).to_owned();
        let body: Vec<usize> = (first + 1..last)
            .filter(|index| !lines[*index].trim().is_empty() && !gone.contains(index))
            .collect();
        let Some(&head) = body.first() else { continue };
        let mut common = white_space(&lines[head]).to_owned();
        for index in &body {
            let line = white_space(&lines[*index]);
            let shared = common
                .chars()
                .zip(line.chars())
                .take_while(|(a, b)| a == b)
                .count();
            common.truncate(shared);
        }
        let Some(extra) = common.strip_prefix(&own).filter(|extra| !extra.is_empty()) else {
            continue;
        };
        let extra = extra.to_owned();
        let prefix = format!("{own}{extra}");
        for line in lines.iter_mut().take(last).skip(first + 1) {
            if let Some(rest) = line.strip_prefix(&prefix) {
                *line = format!("{own}{rest}");
            }
        }
    }
    // Each tag that stays is a mark between two blank lines, led as the line was.
    let mut tags: Vec<(BlockTag, String)> = Vec::new();
    let mut put: Vec<Option<usize>> = vec![None; lines.len()];
    let mut drop = vec![false; lines.len()];
    for line in &gone {
        drop[*line] = true;
    }
    let found_lines: Vec<usize> = found.iter().map(|(index, _)| *index).collect();
    let keep_flags = keep.clone();
    for (at, (index, read)) in found.into_iter().enumerate() {
        if !keep[at] {
            continue;
        }
        if let Line::Tag(tag) = read {
            put[index] = Some(tags.len());
            tags.push((tag, lines[index].trim().to_owned()));
        }
    }
    // What is of the set and stays as words, once each; a summary that was taken as a title is no such thing.
    let mut unmatched: Vec<String> = Vec::new();
    for (at, name) in names.iter().enumerate() {
        let consumed = gone.contains(&found_lines[at]);
        if !keep_flags[at] && !consumed && !unmatched.contains(name) {
            unmatched.push(name.clone());
        }
    }
    if tags.is_empty() {
        return (!unmatched.is_empty()).then(|| Scanned {
            text: text.to_owned(),
            tags,
            mark,
            unmatched,
        });
    }
    let mut out = String::with_capacity(text.len() + tags.len() * 8);
    for (index, line) in lines.iter().enumerate() {
        if drop[index] {
            continue;
        }
        if index > 0 {
            out.push('\n');
        }
        match put[index] {
            Some(number) => {
                let lead = leading(line);
                let blank = lead.trim_end();
                let _ = write!(out, "{blank}\n{lead}{mark}{number}{mark}\n{blank}");
            }
            None => out.push_str(line),
        }
    }
    Some(Scanned {
        text: out,
        tags,
        mark,
        unmatched,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tag_is_read_with_its_attributes_and_what_follows_it() {
        let tag = parse_tag("<column ratio=\"0.4\" Hidden>rest").unwrap();
        assert_eq!(tag.name, "column");
        assert_eq!(tag.attr("ratio"), Some("0.4"));
        assert_eq!(tag.attr("hidden"), Some(""));
        assert_eq!(tag.rest, "rest");
        assert!(parse_tag("<page-break/>").unwrap().self_closing);
        assert!(parse_tag("</details>").unwrap().closing);
        assert!(parse_tag("< b>").is_none());
        assert!(parse_tag("<a href=\"x").is_none());
    }

    #[test]
    fn tags_that_open_and_close_in_order_are_marked_and_the_rest_stay_words() {
        let scanned = scan(
            "<details>\n<summary>More</summary>\n\nbody\n\n</details>\n\n</columns>\n\n<columns>\n",
        )
        .unwrap();
        assert_eq!(scanned.tags.len(), 2, "{:?}", scanned.tags);
        assert_eq!(
            scanned.tags[0].0,
            BlockTag::Toggle {
                summary: Some("More".to_owned())
            }
        );
        assert_eq!(scanned.tags[1].0, BlockTag::Close(Container::Toggle));
        assert!(scanned.text.contains("</columns>"));
        assert!(scanned.text.contains("<columns>"));
        assert!(!scanned.text.contains("<summary>"));
    }

    #[test]
    fn code_is_not_scanned_and_an_indented_body_is_taken_to_its_tag() {
        let text = "```html\n<details>\n</details>\n```\n\n<columns>\n\t<column ratio=\"2\">\n\t\tleft\n\n\t\tmore\n\t</column>\n\t<column>\n\t\tright\n\t</column>\n</columns>\n";
        let scanned = scan(text).unwrap();
        assert_eq!(scanned.tags.len(), 6, "{:?}", scanned.tags);
        assert!(scanned.text.contains("```html\n<details>\n</details>\n```"));
        assert!(scanned.text.contains("\nleft\n"), "{}", scanned.text);
        assert!(scanned.text.contains("\nmore\n"), "{}", scanned.text);
        assert_eq!(scanned.tags[1].0, BlockTag::Column { share: Some(2.0) });
    }

    #[test]
    fn a_mark_that_stands_for_a_tag_is_put_back_as_the_line_it_was() {
        let scanned = scan("a\n\n<page-break/>\n\nb\n").unwrap();
        let mark = scanned.mark;
        let line = format!("{mark}0{mark}");
        assert_eq!(scanned.mark_at(&line), Some(0));
        assert_eq!(
            scanned.restored(&format!("x {line} y")),
            "x <page-break/> y"
        );
    }
}
