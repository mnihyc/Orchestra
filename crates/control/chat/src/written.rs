//! A chat written out: its messages by number with what each carries, in brief or whole, for an agent that reads it, a copy and a download.
//!
//! A chat has one written form, made here and nowhere else, so that whoever
//! learned to read it has learned to read it wherever it turns up. Its parts
//! are its messages, counted from 1 in the order they were written; a message
//! that was deleted is left out, and its number stays its own, so that the
//! numbers of the others never change. A range of the numbers is asked for
//! with `from` and `to`, a negative number counting from the end. The detail is
//! `brief` (a message in one line: who, when and its opening), or `normal` and
//! `full`, which for a chat are the same: the messages, whole.
//!
//! Text is Markdown, with times in UTC and people by their display name and mention handle.
//! Everything was masked when it was stored, so nothing is masked again. What a
//! message carries is one line each, the files lying under the directory beside
//! the text that holds them (`chat.files/` where the text is `chat.md`) by the
//! number of their message in four digits, their carried-file number and their name.

use orch_api_contract::chats::{ChatMessage, ChatMessageKind, ChatPart, ChatSystemEvent};
use orch_api_contract::documents::Author;
use orch_api_contract::threads::{ForwardedAuthor, ForwardedFrom};
use orch_types::Id;
use std::collections::HashMap;
use std::fmt::Write as _;

/// The directory the files of a chat lie in, beside its written form, where the text lies as `chat.md`.
pub const FILES_DIRECTORY: &str = "chat.files";

/// The characters of a message's opening that a message in brief shows.
pub const BRIEF_OPENING_CHARS: usize = 120;

/// A person an agent already reads about, with the handle individual mentions
/// resolve. Missing handles are never invented for a removed identity.
#[must_use]
pub fn person_name(display_name: &str, username: &str) -> String {
    if username.is_empty() {
        display_name.to_owned()
    } else {
        format!("{display_name} (@{username})")
    }
}

/// How much of each message is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Detail {
    /// A message in one line.
    Brief,
    /// The messages, whole.
    #[default]
    Normal,
    /// The same as normal, for a chat.
    Full,
}

/// The messages `from` to `to`, counted in, of a chat that has `last`
/// messages, as the numbers they come to. None where the range holds none. A
/// negative number counts from the end: `-3` is the last three. With neither
/// given, the latest `fit` messages; with `from` alone, `fit` messages forward
/// from there; with `to` alone, `fit` messages ending there.
#[must_use]
pub fn resolve_range(
    from: Option<i64>,
    to: Option<i64>,
    last: i64,
    fit: i64,
) -> Option<(i64, i64)> {
    if last < 1 || fit < 1 {
        return None;
    }
    let number = |n: i64| if n < 0 { last + 1 + n } else { n };
    let (first, end) = match (from.map(number), to.map(number)) {
        (None, None) => (last - fit + 1, last),
        (Some(first), None) => (first, first + fit - 1),
        (None, Some(end)) => (end - fit + 1, end),
        (Some(first), Some(end)) => (first, end),
    };
    let (first, end) = (first.max(1), end.min(last));
    (first <= end).then_some((first, end))
}

/// How many of the latest messages fit in `budget` characters, given the
/// characters each takes, oldest first: as many as fit, and at least one.
#[must_use]
pub fn latest_that_fit(sizes: &[usize], budget: usize) -> usize {
    let mut used = 0_usize;
    let mut taken = 0_usize;
    for size in sizes.iter().rev() {
        if taken > 0 && used.saturating_add(*size) > budget {
            break;
        }
        used = used.saturating_add(*size);
        taken += 1;
    }
    taken
}

/// What the written form needs to know beside the messages.
#[derive(Debug, Clone, Copy)]
pub struct Written<'a> {
    /// The chat's name, which the first line gives: `Chat of Fix login`.
    pub title: &'a str,
    /// The number of the first message given.
    pub first: i64,
    /// The number of the chat's last message.
    pub last: i64,
    /// The numbers of the messages that those given reply to, by identifier,
    /// so that `answers #n` can be said.
    pub numbers: &'a HashMap<Id, i64>,
    /// The directory the files lie in, as the text names it, where it is not
    /// [`FILES_DIRECTORY`]: the one beside the file the text is kept in.
    pub files: Option<&'a str>,
}

/// How many of one unit of a size make the next one up.
const UNIT_RATIO: u32 = 1024;
/// From this many of a unit on, a size is shown without a decimal: three
/// digits say enough.
const WHOLE_FROM: f64 = 100.0;

/// A size as people say it: `212 KiB`, `2.1 MiB`.
#[must_use]
pub fn size_text(bytes: i64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    if bytes < i64::from(UNIT_RATIO) {
        return format!("{bytes} B");
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "a size is shown to three digits, which a double holds for any file there is"
    )]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= f64::from(UNIT_RATIO) && unit + 1 < UNITS.len() {
        value /= f64::from(UNIT_RATIO);
        unit += 1;
    }
    if value >= WHOLE_FROM {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The path of a file a message carries, under the directory of the chat's
/// files: its message number, its carried-file number (starting at one, including
/// forwarded files), and its name with whatever would break a path replaced.
#[must_use]
pub fn file_path(number: i64, carried: usize, name: &str) -> String {
    file_path_in(FILES_DIRECTORY, number, carried, name)
}

/// The same, under the directory `directory` that holds the files beside the
/// text, as the text names it.
#[must_use]
pub fn file_path_in(directory: &str, number: i64, carried: usize, name: &str) -> String {
    format!("{directory}/{}", file_name(number, carried, name))
}

/// The name of a file a message carries in the directory that holds the files
/// of the chat: the message and carried-file numbers followed by its cleaned name.
/// The carried-file number keeps repeated names and names that clean alike distinct.
#[must_use]
pub fn file_name(number: i64, carried: usize, name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | '\0') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    format!("{number:04}-{carried:02}-{clean}")
}

fn who(message: &ChatMessage) -> String {
    match &message.author {
        Some(Author::User(user)) => person_name(&user.display_name, &user.username),
        Some(Author::Agent(agent)) => format!(
            "{} (thread {})",
            agent.agent,
            agent.thread.name.as_deref().unwrap_or("unknown")
        ),
        None => "Orch".to_owned(),
    }
}

fn when(message: &ChatMessage) -> String {
    message
        .created_at
        .get()
        .format("%Y-%m-%dT%H:%MZ")
        .to_string()
}

fn quote(text: &str) -> String {
    text.lines()
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn source_of(from: &ForwardedFrom) -> String {
    match from {
        ForwardedFrom::Chat { chat } => chat
            .node
            .as_ref()
            .and_then(|node| node.name.clone())
            .or_else(|| chat.name.clone())
            .map_or_else(|| "a chat".to_owned(), |name| format!("{name} chat")),
        ForwardedFrom::Thread { thread } => thread
            .name
            .clone()
            .map_or_else(|| "a thread".to_owned(), |name| format!("thread {name}")),
    }
}

fn opening(text: &str) -> String {
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    first.chars().take(BRIEF_OPENING_CHARS).collect()
}

/// What a message says in one line of text: its text parts, the names of what
/// it carries, and the text of the copies it holds.
fn said(message: &ChatMessage) -> String {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            ChatPart::Text { text } if !text.trim().is_empty() => Some(text.trim().to_owned()),
            ChatPart::Forwarded(copy) if !copy.text.trim().is_empty() => {
                Some(copy.text.trim().to_owned())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn system_line(message: &ChatMessage) -> Option<String> {
    let event = message.system.as_ref()?;
    let by = event.by.as_ref().map_or_else(
        || "Someone".to_owned(),
        |by| person_name(&by.display_name, &by.username),
    );
    let user = event.user.as_ref().map_or_else(
        || "someone".to_owned(),
        |user| person_name(&user.display_name, &user.username),
    );
    Some(match event.event {
        ChatSystemEvent::Created => format!("{by} started the chat"),
        ChatSystemEvent::Renamed => format!(
            "{by} named the chat {}",
            event.name.as_deref().unwrap_or("after its members")
        ),
        ChatSystemEvent::MemberAdded => format!("{by} added {user}"),
        ChatSystemEvent::MemberLeft => format!("{by} left"),
        ChatSystemEvent::MemberRemoved => format!("{by} removed {user}"),
        ChatSystemEvent::MemberExpired => format!("{user}'s membership ended"),
    })
}

/// A chat, or a stretch of it, written out: `messages` are the ones asked for,
/// oldest first, the first having the number `written.first`.
#[must_use]
pub fn render(written: &Written<'_>, messages: &[ChatMessage], detail: Detail) -> String {
    let count = i64::try_from(messages.len()).unwrap_or(i64::MAX);
    let to = written.first + count - 1;
    let mut out = format!(
        "# {}\nChat · messages {}–{} of {}\n",
        written.title, written.first, to, written.last
    );
    out.push_str(&render_body(written, messages, detail));
    out
}

/// The same messages without the heading: successive windows of an export
/// append their bodies under one heading, preserving message and file numbers.
#[must_use]
pub fn render_body(written: &Written<'_>, messages: &[ChatMessage], detail: Detail) -> String {
    let directory = written.files.unwrap_or(FILES_DIRECTORY);
    let mut out = String::new();
    for (number, message) in (written.first..).zip(messages) {
        if message.deleted {
            continue;
        }
        out.push('\n');
        if message.kind == ChatMessageKind::System {
            let _ = writeln!(
                out,
                "#{number} · {}",
                system_line(message).unwrap_or_default()
            );
            continue;
        }
        let mut head = format!("#{number} · {} · {}", who(message), when(message));
        if let Some(reply) = &message.reply_to
            && let Some(target) = written.numbers.get(&reply.id)
        {
            let _ = write!(head, " · answers #{target}");
        }
        if message.edited_at.is_some() {
            head.push_str(" · edited");
        }
        if detail == Detail::Brief {
            let text = opening(&said(message));
            let _ = writeln!(
                out,
                "{head}{}",
                if text.is_empty() {
                    String::new()
                } else {
                    format!(" · {text}")
                }
            );
            continue;
        }
        let _ = writeln!(out, "{head}");
        let mut carried = 0_usize;
        for part in &message.parts {
            match part {
                ChatPart::Mention { .. } => {
                    // Normal message views supply the current human label as a text part.
                    let _ = writeln!(out, "(a named audience)");
                }
                ChatPart::Text { text } => {
                    let _ = writeln!(out, "{text}");
                }
                ChatPart::Image { name, .. } => {
                    carried += 1;
                    let _ = writeln!(
                        out,
                        "[image: {name} · {}]",
                        file_path_in(directory, number, carried, name)
                    );
                }
                ChatPart::PastedText(paste) => {
                    carried += 1;
                    let _ = writeln!(
                        out,
                        "[Pasted text: {} characters · {} · {}]",
                        paste.characters,
                        size_text(paste.size),
                        file_path_in(directory, number, carried, &paste.name)
                    );
                }
                ChatPart::Attachment { name, size, .. } => {
                    carried += 1;
                    let _ = writeln!(
                        out,
                        "[file: {name} · {} · {}]",
                        size_text(*size),
                        file_path_in(directory, number, carried, name)
                    );
                }
                ChatPart::ItemRef { .. } => {
                    let _ = writeln!(out, "(quotes an item of the conversation)");
                }
                ChatPart::Forwarded(copy) => {
                    let author = match &copy.author {
                        Some(ForwardedAuthor::User(user)) => {
                            person_name(&user.display_name, &user.username)
                        }
                        Some(ForwardedAuthor::Agent(agent)) => agent.agent.clone(),
                        None => "someone".to_owned(),
                    };
                    let _ = writeln!(
                        out,
                        "> Forwarded from {} · {author} · {}",
                        source_of(&copy.from),
                        copy.at.get().format("%Y-%m-%dT%H:%MZ")
                    );
                    if !copy.text.is_empty() {
                        let _ = writeln!(out, "{}", quote(&copy.text));
                    }
                    for image in &copy.images {
                        carried += 1;
                        let _ = writeln!(
                            out,
                            "> [image: {} · {}]",
                            image.name,
                            file_path_in(directory, number, carried, &image.name)
                        );
                    }
                    for file in &copy.attachments {
                        carried += 1;
                        let _ = writeln!(
                            out,
                            "> [file: {} · {} · {}]",
                            file.name,
                            size_text(file.size),
                            file_path_in(directory, number, carried, &file.name)
                        );
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use orch_api_contract::chats::ChatMessageReply;
    use orch_api_contract::users::{Avatar, UserKind, UserRef};
    use orch_types::{Time, new_id};

    fn person(name: &str) -> UserRef {
        UserRef {
            id: new_id(),
            kind: UserKind::Human,
            username: name.to_lowercase(),
            address: None,
            display_name: name.to_owned(),
            avatar: Avatar {
                text: name[..1].to_owned(),
                color: 1,
            },
        }
    }

    fn message(author: &UserRef, minute: u32, parts: Vec<ChatPart>) -> ChatMessage {
        ChatMessage {
            mention_names: Vec::new(),
            id: new_id(),
            chat_id: new_id(),
            kind: ChatMessageKind::Message,
            author: Some(Author::User(author.clone())),
            parts,
            system: None,
            reply_to: None,
            mentions: Vec::new(),
            mentions_all: false,
            mention_targets: Vec::new(),
            asked_agents: Vec::new(),
            reactions: Vec::new(),
            pinned: None,
            edited_at: None,
            deleted: false,
            seq_created: 1,
            seq_updated: 1,
            created_at: Time::new(
                Utc.with_ymd_and_hms(2026, 10, 9, 14, minute, 0)
                    .single()
                    .unwrap(),
            ),
        }
    }

    fn text(text: &str) -> ChatPart {
        ChatPart::Text {
            text: text.to_owned(),
        }
    }

    #[test]
    fn the_files_of_a_chat_are_named_in_the_directory_it_is_given_by_the_number_of_their_message() {
        let bo = person("Bo");
        let carried = message(
            &bo,
            3,
            vec![
                ChatPart::Attachment {
                    blob: orch_types::Hash::of(b"dump"),
                    name: "dump.sql".to_owned(),
                    size: 2_202_010,
                    media_type: "application/sql".to_owned(),
                },
                ChatPart::Image {
                    blob: orch_types::Hash::of(b"shot"),
                    media_type: orch_api_contract::threads::ImageMediaType::Png,
                    name: "a/screen.png".to_owned(),
                },
            ],
        );
        let numbers = HashMap::new();
        let written = |files| {
            render(
                &Written {
                    title: "Chat of X",
                    first: 4,
                    last: 4,
                    numbers: &numbers,
                    files,
                },
                std::slice::from_ref(&carried),
                Detail::Normal,
            )
        };
        let beside = written(Some("Bo and Dana.files"));
        assert!(
            beside.contains("[file: dump.sql · 2.1 MiB · Bo and Dana.files/0004-01-dump.sql]"),
            "{beside}"
        );
        assert!(
            beside.contains("[image: a/screen.png · Bo and Dana.files/0004-02-a_screen.png]"),
            "{beside}"
        );
        let plain = written(None);
        assert!(plain.contains("· chat.files/0004-01-dump.sql]"), "{plain}");
        assert_eq!(file_name(4, 2, "a/screen.png"), "0004-02-a_screen.png");
        assert_eq!(
            file_path_in("chat.files", 4, 2, "a/screen.png"),
            file_path(4, 2, "a/screen.png")
        );
    }

    #[test]
    fn a_chat_is_written_with_its_messages_by_number_and_what_each_carries() {
        let (dana, bo) = (person("Dana"), person("Bo"));
        let first = message(
            &dana,
            2,
            vec![text("Can someone check the staging database?")],
        );
        let mut second = message(
            &bo,
            3,
            vec![
                text("On it."),
                ChatPart::Attachment {
                    blob: orch_types::Hash::of(b"dump"),
                    name: "dump.sql".to_owned(),
                    size: 2_202_010,
                    media_type: "application/sql".to_owned(),
                },
            ],
        );
        second.reply_to = Some(ChatMessageReply {
            id: first.id,
            author: Some(Author::User(dana.clone())),
            excerpt: String::new(),
            deleted: false,
        });
        let numbers = HashMap::from([(first.id, 120)]);
        let written = render(
            &Written {
                title: "Chat of Fix login",
                first: 120,
                last: 131,
                numbers: &numbers,
                files: None,
            },
            &[first, second],
            Detail::Normal,
        );
        assert_eq!(
            written,
            "# Chat of Fix login\n\
             Chat · messages 120–121 of 131\n\
             \n\
             #120 · Dana (@dana) · 2026-10-09T14:02Z\n\
             Can someone check the staging database?\n\
             \n\
             #121 · Bo (@bo) · 2026-10-09T14:03Z · answers #120\n\
             On it.\n\
             [file: dump.sql · 2.1 MiB · chat.files/0121-01-dump.sql]\n"
        );
    }

    #[test]
    fn a_deleted_message_is_left_out_and_its_number_stays_its_own() {
        let dana = person("Dana");
        let kept = message(&dana, 1, vec![text("first")]);
        let mut gone = message(&dana, 2, vec![]);
        gone.deleted = true;
        let last = message(&dana, 3, vec![text("third")]);
        let written = render(
            &Written {
                title: "Chat of X",
                first: 1,
                last: 3,
                numbers: &HashMap::new(),
                files: None,
            },
            &[kept, gone, last],
            Detail::Normal,
        );
        assert!(written.contains("#1 ·") && written.contains("#3 ·"));
        assert!(!written.contains("#2 ·"));
    }

    #[test]
    fn in_brief_a_message_is_one_line_with_the_opening_of_what_it_says() {
        let dana = person("Dana");
        let long = message(
            &dana,
            2,
            vec![text(&format!("Opening line\n{}", "x".repeat(500)))],
        );
        let written = render(
            &Written {
                title: "Chat of X",
                first: 7,
                last: 7,
                numbers: &HashMap::new(),
                files: None,
            },
            &[long],
            Detail::Brief,
        );
        assert!(written.contains("#7 · Dana (@dana) · 2026-10-09T14:02Z · Opening line\n"));
        assert!(!written.contains("xxx"));
    }

    #[test]
    fn a_range_counts_in_both_ends_and_from_the_end_by_a_negative_number() {
        assert_eq!(resolve_range(None, None, 131, 2), Some((130, 131)));
        assert_eq!(
            resolve_range(Some(120), Some(121), 131, 50),
            Some((120, 121))
        );
        assert_eq!(resolve_range(Some(-3), None, 131, 50), Some((129, 131)));
        assert_eq!(resolve_range(Some(10), None, 131, 3), Some((10, 12)));
        assert_eq!(resolve_range(None, Some(5), 131, 3), Some((3, 5)));
        assert_eq!(resolve_range(Some(200), None, 131, 3), None);
        assert_eq!(resolve_range(None, None, 0, 3), None);
        assert_eq!(resolve_range(Some(0), Some(500), 4, 50), Some((1, 4)));
    }

    #[test]
    fn as_many_of_the_latest_as_fit_and_at_least_one() {
        assert_eq!(latest_that_fit(&[50, 50, 50, 50], 120), 2);
        assert_eq!(
            latest_that_fit(&[500], 120),
            1,
            "a part larger than all is given and cut by its reader"
        );
        assert_eq!(latest_that_fit(&[], 120), 0);
    }

    #[test]
    fn sizes_are_said_as_people_say_them() {
        assert_eq!(size_text(512), "512 B");
        assert_eq!(size_text(217_088), "212 KiB");
        assert_eq!(size_text(2_202_010), "2.1 MiB");
    }

    #[test]
    fn a_file_name_cannot_leave_the_directory_of_the_files() {
        assert_eq!(
            file_path(12, 1, "../../etc/passwd"),
            "chat.files/0012-01-.._.._etc_passwd"
        );
        assert_ne!(file_name(12, 1, "a/b"), file_name(12, 2, "a\\b"));
    }
}
