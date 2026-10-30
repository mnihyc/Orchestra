//! A notification, or several together, as the subject and the two bodies of a mail, from the text its channel has for the recipient's language.
//!
//! The worker hands over a finished text: a subject, made of one line, and a
//! body of plain text. A mail carries it twice, as plain text and as HTML
//! made from that text, so that a reader that shows only one of them shows
//! the same thing: the HTML escapes everything, keeps the lines and the blank
//! lines between paragraphs, and turns an address that starts with `http://`
//! or `https://` into a link. Nothing else of a text is ever interpreted.

use orch_notifier_contract::delivery::Delivery;
use orch_notifier_contract::templates::escape_html;

/// What is written where a message has no subject and no notification to take
/// one from.
const SUBJECT_WHEN_NONE: &str = "Notification";

/// The longest a subject is made: a line that long is still read whole in a
/// list of mail, and nothing a sender writes needs more.
const SUBJECT_MAX_CHARS: usize = 200;

/// A mail as it is written: the subject and the two bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composed {
    /// The subject, one line.
    pub subject: String,
    /// The text.
    pub text: String,
    /// The same, as HTML.
    pub html: String,
}

/// The mail a delivery comes to: the subject it carries, or else the title of
/// the notification it tells, and its text twice.
#[must_use]
pub fn compose(delivery: &Delivery) -> Composed {
    let subject = delivery
        .subject
        .as_deref()
        .map(one_line)
        .filter(|subject| !subject.is_empty())
        .or_else(|| {
            delivery
                .notifications
                .last()
                .map(|notification| one_line(&notification.title))
                .filter(|subject| !subject.is_empty())
        })
        .unwrap_or_else(|| SUBJECT_WHEN_NONE.to_owned());
    Composed {
        subject: subject.chars().take(SUBJECT_MAX_CHARS).collect(),
        text: delivery.body.clone(),
        html: html_of(&delivery.body),
    }
}

/// A text on one line: every run of white space, a line break included,
/// becomes one space, so that nothing in it can begin another header.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The address at the start of `text`, if it starts with one, and how many
/// bytes it takes: up to the first white space or character that cannot be in
/// one, without the punctuation that ends a sentence.
fn address_at(text: &str) -> Option<usize> {
    if !(text.starts_with("http://") || text.starts_with("https://")) {
        return None;
    }
    let end = text
        .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\''))
        .unwrap_or(text.len());
    let trimmed = text[..end].trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}']);
    let scheme = if text.starts_with("https://") { 8 } else { 7 };
    (trimmed.len() > scheme).then_some(trimmed.len())
}

/// One line of text as HTML: everything escaped, and the addresses made links.
fn line_html(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    while !rest.is_empty() {
        let next = rest.find("http").unwrap_or(rest.len());
        out.push_str(&escape_html(&rest[..next]));
        rest = &rest[next..];
        if rest.is_empty() {
            break;
        }
        match address_at(rest) {
            Some(length) => {
                let address = escape_html(&rest[..length]);
                out.push_str(&format!("<a href=\"{address}\">{address}</a>"));
                rest = &rest[length..];
            }
            None => {
                out.push_str("http");
                rest = &rest[4..];
            }
        }
    }
    out
}

/// A text as an HTML document: a paragraph for each run of lines, a break
/// between the lines of one.
#[must_use]
pub fn html_of(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut paragraphs: Vec<String> = Vec::new();
    for block in normalized.split("\n\n") {
        let lines: Vec<String> = block
            .split('\n')
            .filter(|line| !line.trim().is_empty())
            .map(line_html)
            .collect();
        if !lines.is_empty() {
            paragraphs.push(format!("<p>{}</p>", lines.join("<br>\n")));
        }
    }
    format!(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"></head><body style=\"font-family: sans-serif; font-size: 14px; line-height: 1.5\">\n{}\n</body></html>\n",
        paragraphs.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn delivery(subject: Option<&str>, body: &str, titles: &[&str]) -> Delivery {
        let notifications: Vec<_> = titles
            .iter()
            .enumerate()
            .map(|(n, title)| {
                json!({ "id": format!("0199abcd-1234-7000-8000-0000000000{n:02}"), "kind": "turn.completed",
                        "at": "2026-10-11T10:00:00.000Z", "title": title, "body": "", "url": "",
                        "node": null, "thread_id": null, "chat_id": null, "data": null, "recipient": null })
            })
            .collect();
        serde_json::from_value(json!({
            "id": "0199abcd-1234-7000-8000-0000000000d1", "attempt": 1, "config": {}, "to": {},
            "recipient": null, "language": "en", "subject": subject, "body": body,
            "notifications": notifications
        }))
        .unwrap()
    }

    #[test]
    fn a_mail_carries_its_text_twice_and_the_html_escapes_what_the_text_says() {
        let mail = compose(&delivery(
            Some("[Acme] Turn finished"),
            "Fix <login> & more\nhttps://orch.example/thread/1.\n\nSecond paragraph",
            &[],
        ));
        assert_eq!(mail.subject, "[Acme] Turn finished");
        assert!(mail.text.starts_with("Fix <login> & more"));
        assert!(mail.html.contains("Fix &lt;login&gt; &amp; more<br>"));
        assert!(
            mail.html.contains(
                "<a href=\"https://orch.example/thread/1\">https://orch.example/thread/1</a>."
            ),
            "the full stop that ends the sentence is not part of the link: {}",
            mail.html
        );
        assert_eq!(
            mail.html.matches("<p>").count(),
            2,
            "a blank line starts a paragraph"
        );
        assert!(!mail.html.contains("<login>"));
    }

    #[test]
    fn a_subject_is_one_line_and_comes_from_the_newest_notification_when_there_is_none() {
        assert_eq!(
            compose(&delivery(Some("Two\r\nlines  here"), "x", &[])).subject,
            "Two lines here"
        );
        assert_eq!(
            compose(&delivery(None, "x", &["Older", "Newest"])).subject,
            "Newest"
        );
        assert_eq!(
            compose(&delivery(Some("  "), "x", &[])).subject,
            "Notification"
        );
        let long = "w ".repeat(500);
        assert_eq!(
            compose(&delivery(Some(&long), "x", &[]))
                .subject
                .chars()
                .count(),
            200
        );
    }

    #[test]
    fn only_web_addresses_become_links_and_nothing_else_is_interpreted() {
        let html = html_of("see http://a.example/x?y=1&z=2, javascript:alert(1) httpx http");
        assert!(html.contains("<a href=\"http://a.example/x?y=1&amp;z=2\">"));
        assert!(!html.contains("href=\"javascript"));
        assert!(html.contains("httpx http"));
        assert_eq!(html_of("").matches("<p>").count(), 0);
        let quoted = html_of("\"><script>alert(1)</script> https://x.example/\"onmouseover=\"y");
        assert!(!quoted.contains("<script>"));
        assert!(quoted.contains("https://x.example/</a>"));
    }
}
