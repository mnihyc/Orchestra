//! A notification, or several together, as the subject and the two bodies of a mail, from the text its channel has for the recipient's language.
//!
//! The worker hands over a finished text: a subject, made of one line, and a
//! body of plain text. A mail carries it twice, as plain text and as HTML
//! made from that text in one readable frame, so that a reader that shows only one of them shows
//! the same body and destinations: the HTML escapes everything, keeps the lines and the blank
//! lines between paragraphs, and turns an address that starts with `http://`
//! or `https://` into a link. Nothing else of a text is ever interpreted.

use orch_notifier_contract::delivery::Delivery;
use orch_notifier_contract::templates::escape_html;
use url::Url;

/// What is written where a message has no subject and no notification to take
/// one from.
const SUBJECT_WHEN_NONE: &str = "Notification";

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
    let chinese = delivery.language.to_ascii_lowercase().starts_with("zh");
    let (action, settings) = links(delivery);
    let footer = match (chinese, settings.is_some()) {
        (true, true) => "通知设置",
        (true, false) => "由 Orch 发送",
        (false, true) => "Notification settings",
        (false, false) => "Sent by Orch",
    };
    let mut text = delivery.body.clone();
    if let Some(address) = &action
        && !text.contains(address.as_str())
    {
        text.push_str(&format!("\n\n{address}"));
    }
    text.push_str(&format!("\n\n{footer}"));
    if let Some(address) = &settings {
        text.push_str(&format!("\n{address}"));
    }
    let html = framed(
        &subject,
        &delivery.body,
        action.as_ref(),
        settings.as_ref(),
        chinese,
        footer,
    );
    Composed {
        subject,
        text,
        html,
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

/// The escaped paragraphs shared by every email, including a channel's own text.
fn body_html(text: &str) -> String {
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
    paragraphs.join("\n")
}

/// A button is made only for a complete web address already supplied by the delivery.
fn web_address(text: &str) -> Option<Url> {
    let text = text.trim();
    let length = address_at(text)?;
    Url::parse(&text[..length])
        .ok()
        .filter(|url| matches!(url.scheme(), "https" | "http") && url.host_str().is_some())
}

fn links(delivery: &Delivery) -> (Option<Url>, Option<Url>) {
    let native = delivery
        .notifications
        .iter()
        .rev()
        .find_map(|notice| web_address(&notice.url));
    let supplied = native
        .clone()
        .or_else(|| delivery.body.lines().find_map(web_address));
    let Some(mut action) = supplied else {
        return (None, None);
    };
    let settings = native.map(|mut address| {
        address.set_path("/settings/notifications");
        address.set_query(None);
        address.set_fragment(None);
        address
    });
    if delivery.notifications.len() > 1 && settings.is_some() {
        action.set_path("/inbox");
        action.set_query(None);
        action.set_fragment(None);
    }
    (Some(action), settings)
}

/// One responsive, image-free frame serves account emails, notifications and digests.
fn framed(
    subject: &str,
    body: &str,
    action: Option<&Url>,
    settings: Option<&Url>,
    chinese: bool,
    footer: &str,
) -> String {
    let title = escape_html(subject);
    let preheader = escape_html(
        body.lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or(subject),
    );
    let contents = body_html(body);
    let open = if chinese { "打开" } else { "Open" };
    let button = action.map_or_else(String::new, |url| format!(
        "<div style=\"margin:24px 0\"><a data-mail-action href=\"{}\" style=\"display:inline-block;background:#315bd6;color:#ffffff;text-decoration:none;padding:12px 20px;border-radius:6px;font-weight:600\">{open}</a></div>", escape_html(url.as_str())
    ));
    let footer = settings.map_or_else(
        || escape_html(footer),
        |url| {
            format!(
                "<a href=\"{}\" style=\"color:inherit\">{}</a>",
                escape_html(url.as_str()),
                escape_html(footer)
            )
        },
    );
    format!(
        r#"<!DOCTYPE html>
<html lang="{language}"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><meta name="color-scheme" content="light dark"><title>{title}</title>
<style>@media(prefers-color-scheme:dark){{body,.mail-background{{background:#17191d!important;color:#e8eaed!important}}.mail-content{{background:#202328!important;color:#e8eaed!important}}.mail-muted{{color:#b8bec9!important}}}}</style></head>
<body style="margin:0;padding:0;background:#f4f5f7;color:#202124;font-family:Arial,sans-serif;font-size:16px;line-height:1.6">
<div data-mail-preheader style="display:none;max-height:0;overflow:hidden;opacity:0;color:transparent">{preheader}</div>
<table role="presentation" class="mail-background" width="100%" style="border-collapse:collapse;background:#f4f5f7"><tr><td align="center" style="padding:24px 12px">
<table role="presentation" class="mail-content" width="100%" style="max-width:600px;border-collapse:collapse;background:#ffffff;border-radius:8px"><tr><td style="padding:28px 24px;overflow-wrap:anywhere;word-break:break-word">
<div class="mail-muted" style="font-size:14px;color:#5f6672;margin-bottom:16px">Orch</div>
<h1 style="font-size:22px;line-height:1.3;margin:0 0 20px;font-weight:600">{title}</h1>
{contents}{button}
<div class="mail-muted" style="border-top:1px solid #d8dce3;margin-top:24px;padding-top:16px;color:#5f6672;font-size:13px">{footer}</div>
</td></tr></table></td></tr></table></body></html>
"#,
        language = if chinese { "zh-CN" } else { "en" }
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
                        "at": "2026-10-11T10:00:00.000Z", "title": title, "body": "", "url": "https://orch.example/thread/one",
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
    fn a_mail_has_a_readable_frame_one_action_and_the_same_settings_link_in_both_parts() {
        let mail = compose(&delivery(
            Some("[Acme] <Turn finished>"),
            "The work is ready.\nhttps://orch.example/thread/one",
            &["Turn finished"],
        ));
        assert!(mail.html.contains("<h1"));
        assert!(mail.html.contains("[Acme] &lt;Turn finished&gt;"));
        assert!(mail.html.contains("max-width:600px"));
        assert!(mail.html.contains("data-mail-preheader"));
        assert_eq!(mail.html.matches("data-mail-action").count(), 1);
        assert!(mail.html.contains("Notification settings"));
        for body in [&mail.text, &mail.html] {
            assert!(body.contains("https://orch.example/settings/notifications"));
        }
        assert!(!mail.html.contains("<img"));
    }

    #[test]
    fn account_mail_keeps_its_link_and_has_a_localized_footer() {
        for (language, footer) in [("en", "Sent by Orch"), ("zh-CN", "由 Orch 发送")] {
            let mut message = delivery(
                Some("Confirm your address"),
                "https://orch.example/verify-email/example-token",
                &[],
            );
            message.language = language.to_owned();
            let mail = compose(&message);
            assert!(
                mail.html
                    .contains("https://orch.example/verify-email/example-token")
            );
            assert!(mail.text.contains(footer));
            assert!(mail.html.contains(footer));
            assert!(!mail.html.contains("/settings/notifications"));
            assert_eq!(mail.html.matches("data-mail-action").count(), 1);
        }
    }

    #[test]
    fn an_email_uses_only_a_notification_origin_for_its_settings_link() {
        let mut message = delivery(
            Some("Test email"),
            "https://elsewhere.example/information",
            &["Notice"],
        );
        message.notifications[0].url.clear();
        let mail = compose(&message);
        assert!(!mail.html.contains("/settings/notifications"));
        assert!(!mail.text.contains("/settings/notifications"));
        assert!(mail.html.contains("Sent by Orch"));
    }

    #[test]
    fn a_mail_never_turns_an_untrusted_scheme_into_its_action() {
        let mail = compose(&delivery(None, "javascript:alert(1)", &[]));
        assert!(!mail.html.contains("data-mail-action"));
        assert!(!mail.html.contains("href=\"javascript:"));
        assert!(mail.text.contains("javascript:alert(1)"));
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
        let long = "完整标题 w ".repeat(500);
        assert_eq!(
            compose(&delivery(Some(&long), "x", &[])).subject,
            long.trim()
        );
    }

    #[test]
    fn only_web_addresses_become_links_and_nothing_else_is_interpreted() {
        let html = body_html("see http://a.example/x?y=1&z=2, javascript:alert(1) httpx http");
        assert!(html.contains("<a href=\"http://a.example/x?y=1&amp;z=2\">"));
        assert!(!html.contains("href=\"javascript"));
        assert!(html.contains("httpx http"));
        assert_eq!(body_html("").matches("<p>").count(), 0);
        let quoted = body_html("\"><script>alert(1)</script> https://x.example/\"onmouseover=\"y");
        assert!(!quoted.contains("<script>"));
        assert!(quoted.contains("https://x.example/</a>"));
    }
}
