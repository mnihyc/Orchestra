//! What a skill's `SKILL.md` says of itself: its front matter read, and its name and description checked against the format.
//!
//! The front matter is YAML between a first line `---` and the next line `---`. Orch reads the
//! members it needs (`name`, `description`, and `compatibility` where it is given) and keeps the rest
//! of the file as it is: the agent programs read the file themselves, and some of them define members
//! of their own. A value is read the way a YAML parser reads it (plain, quoted, or a block of lines),
//! and a value that a YAML parser would refuse is refused here too, with the reason, so that a version
//! Orch keeps is one the agent programs can load.

use std::collections::BTreeSet;

/// The most characters a skill's name has, as the format says.
pub const NAME_MAX_CHARS: usize = 64;
/// The most characters a skill's description has, as the format says.
pub const DESCRIPTION_MAX_CHARS: usize = 1024;
/// The most characters the optional `compatibility` member has, as the format says.
pub const COMPATIBILITY_MAX_CHARS: usize = 500;

/// What a skill says of itself in its front matter, as far as Orch reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontMatter {
    /// Its name: lower-case letters, digits and single hyphens inside.
    pub name: String,
    /// What it does and when to use it.
    pub description: String,
}

/// Why the front matter of a `SKILL.md` was refused, in the words a person reads. The words name
/// no file and quote nothing written: what a refusal is about (a line, a member, a name) is given by
/// [`FrontMatterFault::about`], since a reply carries what was written beside its words, not in them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrontMatterFault {
    /// The file does not begin with a front matter.
    #[error("The skill file does not begin with a front matter: its first line has to be ---")]
    Missing,
    /// The front matter never ends.
    #[error("The front matter of the skill file does not end: a line --- has to close it")]
    Unclosed,
    /// A line of the front matter is not YAML that Orch reads.
    #[error(
        "Line {line} of the front matter is not a member written as a name, a colon and a value"
    )]
    NotAMember {
        /// The line, counted from the first line of the file.
        line: usize,
        /// The line, cut short.
        text: String,
    },
    /// A member is given twice.
    #[error("The front matter gives a member twice")]
    Twice {
        /// The member.
        member: String,
    },
    /// A member that has to be there is not.
    #[error("The front matter gives no {member}")]
    Absent {
        /// The member.
        member: &'static str,
    },
    /// A member is not text.
    #[error("The {member} in the front matter has to be text")]
    NotText {
        /// The member.
        member: &'static str,
    },
    /// A plain value holds what YAML reads as something else.
    #[error("The {member} in the front matter has to be put in quotes: {why}")]
    NeedsQuotes {
        /// The member.
        member: &'static str,
        /// What in it a YAML parser would not read as text.
        why: &'static str,
    },
    /// A quoted value is not closed.
    #[error("The {member} in the front matter opens a quote and does not close it")]
    QuoteOpen {
        /// The member.
        member: &'static str,
    },
    /// The name does not follow the format.
    #[error("The name in the front matter {why}")]
    Name {
        /// The name as written.
        name: String,
        /// What is wrong with it.
        why: &'static str,
    },
    /// A member is longer than the format allows.
    #[error("The {member} in the front matter has {chars} characters; at most {max} are allowed")]
    TooLong {
        /// The member.
        member: &'static str,
        /// How many characters it has.
        chars: usize,
        /// The most it may have.
        max: usize,
    },
}

impl FrontMatterFault {
    /// What the refusal is about, as it was written: the line, the member given twice or the name.
    #[must_use]
    pub fn about(&self) -> Vec<String> {
        match self {
            Self::NotAMember { text, .. } => vec![text.clone()],
            Self::Twice { member } => vec![member.clone()],
            Self::Name { name, .. } => vec![name.clone()],
            Self::Missing
            | Self::Unclosed
            | Self::Absent { .. }
            | Self::NotText { .. }
            | Self::NeedsQuotes { .. }
            | Self::QuoteOpen { .. }
            | Self::TooLong { .. } => Vec::new(),
        }
    }
}

/// One top-level member of the front matter: the text after its colon on its own line, and the
/// lines below it that are indented, as they were written.
struct Member<'a> {
    first: &'a str,
    more: Vec<&'a str>,
}

/// Reads the front matter of a `SKILL.md` and checks its name and description.
pub fn read(text: &str) -> Result<FrontMatter, FrontMatterFault> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line));
    if lines.next().map(str::trim_end) != Some("---") {
        return Err(FrontMatterFault::Missing);
    }
    let mut body: Vec<&str> = Vec::new();
    let mut closed = false;
    for line in lines {
        let end = line.trim_end();
        if end == "---" || end == "..." {
            closed = true;
            break;
        }
        body.push(line);
    }
    if !closed {
        return Err(FrontMatterFault::Unclosed);
    }
    let members = members(&body)?;
    let name = scalar(&members, "name")?.ok_or(FrontMatterFault::Absent { member: "name" })?;
    let description = scalar(&members, "description")?
        .filter(|text| !text.trim().is_empty())
        .ok_or(FrontMatterFault::Absent {
            member: "description",
        })?;
    if let Some(why) = name_fault(&name) {
        return Err(FrontMatterFault::Name { name, why });
    }
    let chars = description.chars().count();
    if chars > DESCRIPTION_MAX_CHARS {
        return Err(FrontMatterFault::TooLong {
            member: "description",
            chars,
            max: DESCRIPTION_MAX_CHARS,
        });
    }
    if let Some(compatibility) = scalar(&members, "compatibility")? {
        let chars = compatibility.chars().count();
        if chars > COMPATIBILITY_MAX_CHARS {
            return Err(FrontMatterFault::TooLong {
                member: "compatibility",
                chars,
                max: COMPATIBILITY_MAX_CHARS,
            });
        }
    }
    Ok(FrontMatter { name, description })
}

/// What is wrong with `name` as the name of a skill, if anything: one to 64 lower-case letters,
/// digits and hyphens, with no hyphen at either end and never two together.
#[must_use]
pub fn name_fault(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        return Some("is empty");
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Some("is longer than 64 characters");
    }
    if !name
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Some("may hold only lower-case letters, digits and hyphens");
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Some("may not begin or end with a hyphen");
    }
    if name.contains("--") {
        return Some("may not hold two hyphens together");
    }
    None
}

/// The top-level members of the front matter's lines, each with the indented lines below it.
fn members<'a>(body: &[&'a str]) -> Result<Vec<(&'a str, Member<'a>)>, FrontMatterFault> {
    let mut found: Vec<(&'a str, Member<'a>)> = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (index, line) in body.iter().enumerate() {
        // The first line of the file is the opening `---`.
        let number = index + 2;
        let indented = line.starts_with([' ', '\t']);
        if indented || line.trim().is_empty() {
            if let Some((_, member)) = found.last_mut() {
                member.more.push(line);
            } else if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
                return Err(not_a_member(number, line));
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let Some((key, rest)) = split_key(line) else {
            return Err(not_a_member(number, line));
        };
        if !seen.insert(key) {
            return Err(FrontMatterFault::Twice {
                member: key.to_owned(),
            });
        }
        found.push((
            key,
            Member {
                first: rest,
                more: Vec::new(),
            },
        ));
    }
    Ok(found)
}

fn not_a_member(line: usize, text: &str) -> FrontMatterFault {
    /// How much of a refused line is quoted back: enough to find it.
    const QUOTED_CHARS: usize = 80;
    FrontMatterFault::NotAMember {
        line,
        text: text.chars().take(QUOTED_CHARS).collect(),
    }
}

/// A line `key: rest` or `key:` split at its colon, where the key is a plain word.
fn split_key(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let (key, rest) = (&line[..colon], &line[colon + 1..]);
    let plain = !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    (plain && (rest.is_empty() || rest.starts_with([' ', '\t']))).then_some((key, rest))
}

/// The text of the member `key`, read as a YAML scalar; nothing where it is not given or is null.
fn scalar(
    members: &[(&str, Member<'_>)],
    key: &'static str,
) -> Result<Option<String>, FrontMatterFault> {
    let Some((_, member)) = members.iter().find(|(found, _)| *found == key) else {
        return Ok(None);
    };
    let first = strip_comment(member.first.trim());
    let more: Vec<&str> = member.more.clone();
    let continued = more.iter().any(|line| !line.trim().is_empty());
    match first.chars().next() {
        None if !continued => Ok(None),
        None => {
            let lines: Vec<&str> = more.iter().map(|line| line.trim()).collect();
            let opener = lines
                .iter()
                .find(|line| !line.is_empty())
                .copied()
                .unwrap_or("");
            if opener.starts_with("- ") || opener == "-" || split_key(opener).is_some() {
                return Err(FrontMatterFault::NotText { member: key });
            }
            plain(key, &lines).map(Some)
        }
        Some('|' | '>') => block(key, member.first.trim(), &more).map(Some),
        Some('"') => double_quoted(key, member.first.trim_start(), &more).map(Some),
        Some('\'') => single_quoted(key, member.first.trim_start(), &more).map(Some),
        Some('[' | '{') => Err(FrontMatterFault::NotText { member: key }),
        Some('&' | '*' | '!' | '%' | '@' | '`' | ',' | '?' | '#') => {
            Err(FrontMatterFault::NeedsQuotes {
                member: key,
                why: "it begins with a sign YAML reserves",
            })
        }
        Some('-') if first == "-" || first.starts_with("- ") => {
            Err(FrontMatterFault::NotText { member: key })
        }
        Some(_) => {
            let mut lines: Vec<&str> = vec![first];
            lines.extend(more.iter().map(|line| line.trim()));
            let value = plain(key, &lines)?;
            if matches!(value.as_str(), "~" | "null" | "Null" | "NULL") {
                return Ok(None);
            }
            Ok(Some(value))
        }
    }
}

/// A plain value without its comment: what follows a `#` that follows a space.
fn strip_comment(text: &str) -> &str {
    let bytes = text.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'#' && index > 0 && matches!(bytes[index - 1], b' ' | b'\t') {
            return text[..index].trim_end();
        }
    }
    text
}

/// A plain value over several lines, folded as YAML folds it: lines joined by one space, and an
/// empty line kept as a line break.
fn plain(key: &'static str, lines: &[&str]) -> Result<String, FrontMatterFault> {
    let mut value = String::new();
    let mut breaks = 0usize;
    for line in lines {
        let line = strip_comment(line.trim());
        if line.is_empty() {
            breaks += 1;
            continue;
        }
        if line.contains(": ") || line.ends_with(':') {
            return Err(FrontMatterFault::NeedsQuotes {
                member: key,
                why: "it holds a colon followed by a space",
            });
        }
        if !value.is_empty() {
            if breaks > 0 {
                value.push_str(&"\n".repeat(breaks));
            } else {
                value.push(' ');
            }
        }
        breaks = 0;
        value.push_str(line);
    }
    Ok(value)
}

/// A block value, `|` keeping its line breaks or `>` folding them, with its chomping sign.
fn block(key: &'static str, header: &str, lines: &[&str]) -> Result<String, FrontMatterFault> {
    let header = strip_comment(header);
    let literal = header.starts_with('|');
    let mut keep = false;
    let mut strip = false;
    let mut indent: Option<usize> = None;
    for sign in header.chars().skip(1) {
        match sign {
            '+' => keep = true,
            '-' => strip = true,
            '1'..='9' => indent = sign.to_digit(10).map(|digit| digit as usize),
            _ => {
                return Err(FrontMatterFault::NeedsQuotes {
                    member: key,
                    why: "its block sign is not one YAML knows",
                });
            }
        }
    }
    let indent = indent.unwrap_or_else(|| {
        lines
            .iter()
            .find(|line| !line.trim().is_empty())
            .map_or(0, |line| line.len() - line.trim_start_matches(' ').len())
    });
    let mut kept: Vec<&str> = Vec::new();
    for line in lines {
        if line.trim().is_empty() {
            kept.push("");
            continue;
        }
        let leading = line.len() - line.trim_start_matches(' ').len();
        if leading < indent {
            return Err(FrontMatterFault::NotText { member: key });
        }
        kept.push(&line[indent..]);
    }
    let trailing = kept.iter().rev().take_while(|line| line.is_empty()).count();
    let content = &kept[..kept.len() - trailing];
    let mut value = String::new();
    if literal {
        value.push_str(&content.join("\n"));
    } else {
        // Folded: lines that begin with a space and empty lines keep their breaks.
        let mut previous_more = false;
        for (index, line) in content.iter().enumerate() {
            let more = line.starts_with(' ');
            if index > 0 {
                if line.is_empty() || more || previous_more {
                    value.push('\n');
                } else if !content[index - 1].is_empty() {
                    value.push(' ');
                }
            }
            value.push_str(line);
            previous_more = more;
        }
    }
    if !content.is_empty() && !strip {
        value.push('\n');
        if keep {
            value.push_str(&"\n".repeat(trailing));
        }
    }
    Ok(value)
}

/// A value in double quotes, over one line or several, with its escapes read.
fn double_quoted(
    key: &'static str,
    first: &str,
    more: &[&str],
) -> Result<String, FrontMatterFault> {
    let source = joined(first, more);
    let mut inside = String::new();
    let mut chars = source.chars().skip(1);
    let mut closed = false;
    while let Some(character) = chars.next() {
        match character {
            '"' => {
                closed = true;
                break;
            }
            '\\' => {
                inside.push('\\');
                if let Some(escaped) = chars.next() {
                    inside.push(escaped);
                }
            }
            other => inside.push(other),
        }
    }
    if !closed {
        return Err(FrontMatterFault::QuoteOpen { member: key });
    }
    let rest: String = chars.collect();
    if !strip_comment(rest.trim()).is_empty() {
        return Err(FrontMatterFault::NotText { member: key });
    }
    unescape(key, &fold_quoted(&inside, true))
}

/// The escapes of a double-quoted value read: what each stands for.
fn unescape(key: &'static str, folded: &str) -> Result<String, FrontMatterFault> {
    let refused = FrontMatterFault::NeedsQuotes {
        member: key,
        why: "an escape in its quotes is not one YAML reads",
    };
    let mut value = String::new();
    let mut chars = folded.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            value.push(character);
            continue;
        }
        let Some(escaped) = chars.next() else {
            return Err(refused);
        };
        match escaped {
            'n' => value.push('\n'),
            't' | '\t' => value.push('\t'),
            'r' => value.push('\r'),
            '0' => value.push('\0'),
            '\\' => value.push('\\'),
            '"' => value.push('"'),
            '/' => value.push('/'),
            ' ' => value.push(' '),
            'x' | 'u' | 'U' => {
                let digits = match escaped {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let hex: String = chars.by_ref().take(digits).collect();
                let decoded = (hex.len() == digits)
                    .then(|| u32::from_str_radix(&hex, 16).ok())
                    .flatten()
                    .and_then(char::from_u32);
                let Some(decoded) = decoded else {
                    return Err(refused);
                };
                value.push(decoded);
            }
            _ => return Err(refused),
        }
    }
    Ok(value)
}

/// A value in single quotes, where two quotes stand for one.
fn single_quoted(
    key: &'static str,
    first: &str,
    more: &[&str],
) -> Result<String, FrontMatterFault> {
    let source = joined(first, more);
    let mut chars = source.chars().skip(1).peekable();
    let mut inside = String::new();
    let mut closed = false;
    while let Some(character) = chars.next() {
        if character == '\'' {
            if chars.peek() == Some(&'\'') {
                chars.next();
                inside.push_str("''");
                continue;
            }
            closed = true;
            break;
        }
        inside.push(character);
    }
    if !closed {
        return Err(FrontMatterFault::QuoteOpen { member: key });
    }
    let rest: String = chars.collect();
    if !strip_comment(rest.trim()).is_empty() {
        return Err(FrontMatterFault::NotText { member: key });
    }
    Ok(fold_quoted(&inside, false).replace("''", "'"))
}

/// The first line of a value and the lines below it, as one text with line breaks.
fn joined(first: &str, more: &[&str]) -> String {
    let mut source = first.to_owned();
    for line in more {
        source.push('\n');
        source.push_str(line);
    }
    source
}

/// Line breaks inside quotes folded as YAML folds them: the spaces around a break are dropped, one
/// break becomes a space, and each further (empty) line is kept as a break. In double quotes a
/// backslash at the end of a line joins it to the next without a space.
fn fold_quoted(inside: &str, escapes: bool) -> String {
    let lines: Vec<&str> = inside.split('\n').collect();
    if lines.len() == 1 {
        return inside.to_owned();
    }
    let last = lines.len() - 1;
    let mut value = String::new();
    let mut breaks = 0usize;
    let mut joined_without_space = false;
    for (index, line) in lines.iter().enumerate() {
        let mut line = if index == 0 {
            line.trim_end_matches([' ', '\t'])
        } else if index == last {
            line.trim_start_matches([' ', '\t'])
        } else {
            line.trim_matches([' ', '\t'])
        };
        if index > 0 && index < last && line.is_empty() {
            breaks += 1;
            continue;
        }
        if index > 0 {
            if breaks > 0 {
                value.push_str(&"\n".repeat(breaks));
            } else if !joined_without_space {
                value.push(' ');
            }
        }
        breaks = 0;
        joined_without_space = false;
        if escapes && index < last {
            let backslashes = line.len() - line.trim_end_matches('\\').len();
            if backslashes % 2 == 1 {
                line = &line[..line.len() - 1];
                joined_without_space = true;
            }
        }
        value.push_str(line);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(front: &str) -> String {
        format!("---\n{front}\n---\n# Body\n")
    }

    #[test]
    fn a_plain_name_and_description_are_read() {
        let found = read(&skill(
            "name: pdf-tools\ndescription: Reads PDF files. Use for PDFs.",
        ))
        .unwrap();
        assert_eq!(found.name, "pdf-tools");
        assert_eq!(found.description, "Reads PDF files. Use for PDFs.");
    }

    #[test]
    fn other_members_and_comments_are_left_alone() {
        let found = read(&skill(
            "# made by hand\nname: notes\nlicense: Apache-2.0\nmetadata:\n  author: example\n  version: \"1.0\"\nallowed-tools: Bash(git:*) Read\ndisable-model-invocation: true\ndescription: Takes notes # a comment",
        ))
        .unwrap();
        assert_eq!(found.name, "notes");
        assert_eq!(found.description, "Takes notes");
    }

    #[test]
    fn quoted_values_are_read_with_their_escapes() {
        let found = read(&skill(
            "name: 'quoted'\ndescription: \"Says \\\"hi\\\": then\\tstops\"",
        ))
        .unwrap();
        assert_eq!(found.name, "quoted");
        assert_eq!(found.description, "Says \"hi\": then\tstops");
        let single = read(&skill("name: single\ndescription: 'It''s here: now'")).unwrap();
        assert_eq!(single.description, "It's here: now");
    }

    #[test]
    fn values_over_several_lines_fold_as_yaml_folds_them() {
        let plain = read(&skill(
            "name: long\ndescription: Reads the files\n  of a project\n\n  and more.",
        ))
        .unwrap();
        assert_eq!(plain.description, "Reads the files of a project\nand more.");
        let folded = read(&skill(
            "name: long\ndescription: >\n  One line\n  and another.\n",
        ))
        .unwrap();
        assert_eq!(folded.description, "One line and another.\n");
        let literal = read(&skill("name: long\ndescription: |-\n  First\n  Second")).unwrap();
        assert_eq!(literal.description, "First\nSecond");
        let quoted = read(&skill(
            "name: long\ndescription: \"One\n  two\\\n  three\\nfour\"",
        ))
        .unwrap();
        assert_eq!(quoted.description, "One twothree\nfour");
    }

    #[test]
    fn a_file_without_front_matter_is_refused() {
        assert_eq!(read("# Just text\n"), Err(FrontMatterFault::Missing));
        assert_eq!(
            read("---\nname: x\ndescription: y\n"),
            Err(FrontMatterFault::Unclosed)
        );
    }

    #[test]
    fn a_missing_or_empty_member_is_refused_with_its_name() {
        assert_eq!(
            read(&skill("description: Does things")),
            Err(FrontMatterFault::Absent { member: "name" })
        );
        assert_eq!(
            read(&skill("name: thing\ndescription: \"  \"")),
            Err(FrontMatterFault::Absent {
                member: "description"
            })
        );
        assert_eq!(
            read(&skill("name: ~\ndescription: Does")),
            Err(FrontMatterFault::Absent { member: "name" })
        );
    }

    #[test]
    fn names_follow_the_format() {
        for good in ["a", "pdf-processing", "x1-y2", &"a".repeat(64)] {
            assert_eq!(name_fault(good), None, "{good}");
        }
        for bad in [
            "",
            "PDF",
            "-pdf",
            "pdf-",
            "pdf--processing",
            "pdf_processing",
            "pdf processing",
            "ünï",
            &"a".repeat(65),
        ] {
            assert!(name_fault(bad).is_some(), "{bad}");
        }
        assert!(matches!(
            read(&skill("name: Pdf\ndescription: Does")),
            Err(FrontMatterFault::Name { .. })
        ));
    }

    #[test]
    fn a_description_longer_than_the_format_allows_is_refused() {
        let long = "x".repeat(DESCRIPTION_MAX_CHARS + 1);
        assert_eq!(
            read(&skill(&format!("name: long\ndescription: {long}"))),
            Err(FrontMatterFault::TooLong {
                member: "description",
                chars: DESCRIPTION_MAX_CHARS + 1,
                max: DESCRIPTION_MAX_CHARS
            })
        );
        let fits = "é".repeat(DESCRIPTION_MAX_CHARS);
        assert!(read(&skill(&format!("name: long\ndescription: {fits}"))).is_ok());
        let compatibility = "y".repeat(COMPATIBILITY_MAX_CHARS + 1);
        assert!(matches!(
            read(&skill(&format!(
                "name: c\ndescription: d\ncompatibility: {compatibility}"
            ))),
            Err(FrontMatterFault::TooLong {
                member: "compatibility",
                ..
            })
        ));
    }

    #[test]
    fn what_yaml_would_not_read_as_text_is_refused() {
        assert!(matches!(
            read(&skill("name: x\ndescription: Use when: asked")),
            Err(FrontMatterFault::NeedsQuotes { .. })
        ));
        assert_eq!(
            read(&skill("name: x\ndescription: [a, b]")),
            Err(FrontMatterFault::NotText {
                member: "description"
            })
        );
        assert_eq!(
            read(&skill("name: x\ndescription:\n  - a\n  - b")),
            Err(FrontMatterFault::NotText {
                member: "description"
            })
        );
        assert_eq!(
            read(&skill("name: x\ndescription: \"open")),
            Err(FrontMatterFault::QuoteOpen {
                member: "description"
            })
        );
        assert!(matches!(
            read(&skill("name: x\nname: y\ndescription: d")),
            Err(FrontMatterFault::Twice { .. })
        ));
        assert!(matches!(
            read(&skill("name: x\n just text\ndescription: d")),
            Err(FrontMatterFault::Name { .. })
        ));
        assert!(matches!(
            read(&skill("not a member\nname: x\ndescription: d")),
            Err(FrontMatterFault::NotAMember { line: 2, .. })
        ));
    }

    #[test]
    fn a_byte_order_mark_and_windows_line_ends_are_accepted() {
        let found =
            read("\u{feff}---\r\nname: win\r\ndescription: Made on Windows\r\n---\r\n").unwrap();
        assert_eq!(found.name, "win");
        assert_eq!(found.description, "Made on Windows");
    }
}
