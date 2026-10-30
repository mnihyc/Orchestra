//! Push content and target URLs.
//!
//! What a browser's service worker is sent is a small JSON document, encrypted
//! to that browser: a title, a body, the address to open when it is pressed,
//! and a tag that lets a newer notification take the place of an older one that
//! is still on the screen. It carries what the Inbox row shows and no more: a
//! title, a first line, a link. A message that tells several notifications is
//! one push whose address is the Inbox.
//!
//! A push service takes a few thousand bytes of payload at most, and a
//! notification that is longer than a screen is not read, so the text is cut
//! short, at the end of a character.

use orch_notifier_contract::delivery::Delivery;
use serde::Serialize;

/// The most characters of the body that go to a browser: more than a lock
/// screen shows, and far from the size a push service refuses at.
pub const BODY_CHARS: u32 = 1000;

/// The most characters of the title.
pub const TITLE_CHARS: usize = 200;

/// Where a digest opens, which is the Inbox: the notifications of a digest are
/// not one place.
pub const INBOX_PATH: &str = "/inbox";

/// What the service worker is sent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Content {
    /// Its title.
    pub title: String,
    /// Its text.
    pub body: String,
    /// The address to open when it is pressed.
    pub url: String,
    /// What a newer notification replaces: the notification's identifier, or,
    /// for several together, the message's.
    pub tag: String,
    /// The kind of the notification, or `digest` for several.
    pub kind: String,
    /// How many notifications it tells.
    pub count: usize,
}

fn cut(text: &str, chars: usize) -> String {
    text.chars().take(chars).collect()
}

/// What a delivery comes to for a browser.
#[must_use]
pub fn content_of(delivery: &Delivery) -> Content {
    let newest = delivery.notifications.last();
    let count = delivery.notifications.len().max(1);
    let title = delivery
        .subject
        .as_deref()
        .map(str::trim)
        .filter(|subject| !subject.is_empty())
        .or_else(|| newest.map(|each| each.title.as_str()))
        .unwrap_or("Notification");
    let (url, tag, kind) = match (count, newest) {
        (1, Some(only)) => (
            only.url.clone(),
            only.id.simple().to_string(),
            kind_of(only),
        ),
        (_, newest) => (
            newest.map_or_else(|| INBOX_PATH.to_owned(), |each| inbox_of(&each.url)),
            format!("digest-{}", delivery.id.simple()),
            "digest".to_owned(),
        ),
    };
    Content {
        title: cut(title, TITLE_CHARS),
        body: cut(&delivery.body, BODY_CHARS as usize),
        url,
        tag,
        kind,
        count,
    }
}

fn kind_of(notification: &orch_notifier_contract::delivery::NotificationBody) -> String {
    serde_json::to_value(notification.kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The Inbox on the origin that `address` is on, or the path alone where
/// `address` is not an absolute one.
fn inbox_of(address: &str) -> String {
    match url::Url::parse(address)
        .ok()
        .and_then(|url| url.join(INBOX_PATH).ok())
    {
        Some(inbox) => inbox.to_string(),
        None => INBOX_PATH.to_owned(),
    }
}

/// The topic that lets a push service replace a message it still holds: the
/// notification's identifier, which is 32 characters of a base64 alphabet. A
/// message that tells several has none.
#[must_use]
pub fn topic_of(delivery: &Delivery) -> Option<String> {
    match delivery.notifications.as_slice() {
        [only] => Some(only.id.simple().to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn delivery(subject: Option<&str>, body: &str, titles: &[&str]) -> Delivery {
        let notifications: Vec<_> = titles
            .iter()
            .enumerate()
            .map(|(n, title)| json!({
                "id": format!("0199abcd-1234-7000-8000-0000000000{n:02}"), "kind": "approval.requested",
                "at": "2026-10-11T10:00:00.000Z", "title": title, "body": "", "url": format!("https://orch.example/thread/{n}"),
                "node": null, "thread_id": null, "chat_id": null, "data": {}, "recipient": null }))
            .collect();
        serde_json::from_value(json!({
            "id": "0199abcd-1234-7000-8000-0000000000d1", "attempt": 1, "config": {}, "to": {}, "recipient": null,
            "language": "en", "subject": subject, "body": body, "notifications": notifications
        }))
        .unwrap()
    }

    #[test]
    fn one_notification_opens_its_own_place_and_replaces_itself_when_it_is_sent_again() {
        let one = delivery(
            Some("Approve the deploy?"),
            "Fix login",
            &["Approve the deploy?"],
        );
        let content = content_of(&one);
        assert_eq!(content.title, "Approve the deploy?");
        assert_eq!(content.body, "Fix login");
        assert_eq!(content.url, "https://orch.example/thread/0");
        assert_eq!(content.kind, "approval.requested");
        assert_eq!(content.count, 1);
        assert_eq!(content.tag, "0199abcd123470008000000000000000");
        assert_eq!(topic_of(&one).as_deref(), Some(content.tag.as_str()));
        assert!(
            topic_of(&one).unwrap().len() <= 32,
            "a topic is at most 32 characters"
        );
    }

    #[test]
    fn several_together_open_the_inbox_on_the_same_origin() {
        let several = delivery(
            Some("3 unread"),
            "One\nTwo\nThree",
            &["One", "Two", "Three"],
        );
        let content = content_of(&several);
        assert_eq!(
            (content.url.as_str(), content.kind.as_str(), content.count),
            ("https://orch.example/inbox", "digest", 3)
        );
        assert!(content.tag.starts_with("digest-"));
        assert_eq!(topic_of(&several), None);
        let lone = delivery(None, "", &[]);
        assert_eq!(content_of(&lone).url, "/inbox");
        assert_eq!(content_of(&lone).title, "Notification");
    }

    #[test]
    fn a_title_comes_from_the_subject_else_the_newest_notification_and_long_texts_are_cut() {
        assert_eq!(
            content_of(&delivery(None, "", &["Older", "Newest"])).title,
            "Newest"
        );
        assert_eq!(
            content_of(&delivery(Some("  "), "", &["Only"])).title,
            "Only"
        );
        let long = "é".repeat(5_000);
        let content = content_of(&delivery(Some(&long), &long, &["x"]));
        assert_eq!(content.title.chars().count(), TITLE_CHARS);
        assert_eq!(content.body.chars().count(), BODY_CHARS as usize);
        let json = serde_json::to_string(&content).unwrap();
        assert!(
            json.len() < 3_000,
            "far below what a push service takes: {}",
            json.len()
        );
    }
}
