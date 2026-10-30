//! A message as its notifier gets it: the notifications of a row, the text of the channel or the one the notifier ships, filled in for the language and time zone of the person, and cut to the length a message takes.
//!
//! The engine decides who is told what and writes the notifications into the
//! outbox row as they were. The worker renders: it picks the text of the
//! channel for the person's language, or the one the notifier ships (the
//! channel's own for a language, its own for every language, the shipped one
//! for the language, the shipped one for every language), fills in the closed
//! list of values with each escaped for the notifier's medium, and, where the
//! notifier takes only so many characters, cuts the text at the end of a line
//! and ends it with how many more there are and the address of the Inbox.
//!
//! A notifier whose medium is JSON is sent only the text its channel has of its
//! own: the shipped text of such a notifier is an example to start an editor
//! from. Without a text of its own the body is empty, and the notifier builds
//! the body from the notifications that come with it.

use chrono_tz::Tz;
use orch_db::queries::notifications::outbox::Snapshot;
use orch_db::queries::notifications::people::{PlaceRow, Recipient};
use orch_notifier_contract::delivery::{NotificationBody, NotificationKind};
use orch_notifier_contract::descriptor::NotifierDescriptor;
use orch_notifier_contract::templates::{
    BodyKind, Form, Item, TemplateError, TemplateOverride, Values, fit, resolve,
};
use orch_plugin_contract::refs::{Avatar, NodeKind, NodeRef, UserKind, UserRef};
use orch_types::{Id, Json};
use std::collections::BTreeMap;

/// The language a text is written in for a place that is told for nobody in
/// particular.
pub const DESTINATION_LANGUAGE: &str = "en";

/// Why a message could not be rendered.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RenderFault {
    /// A text does not render.
    #[error("the text does not render: {0}")]
    Template(#[from] TemplateError),
    /// The notifier has no text for the language and none for every language.
    #[error("there is no text to send")]
    NoText,
    /// A notification is of a kind that is not one.
    #[error("{0:?} is not a kind of notification")]
    UnknownKind(String),
}

/// What a message is rendered from.
#[derive(Debug, Clone, Copy)]
pub struct Rendering<'a> {
    /// What the notifier declares.
    pub descriptor: &'a NotifierDescriptor,
    /// The texts of the channel or the destination, by language.
    pub own_texts: &'a Json,
    /// The language to write in.
    pub language: &'a str,
    /// The person it is for, where it is for one.
    pub recipient: Option<&'a Recipient>,
    /// The notifications it tells, oldest first.
    pub notifications: &'a [Snapshot],
    /// The places they happened at.
    pub places: &'a [PlaceRow],
    /// The name of the org a text names: the root org of the newest notification,
    /// or the system where it lies in none.
    pub org_name: &'a str,
    /// The origin a browser opens, which in-app paths are joined to.
    pub public_url: &'a str,
}

/// A message as it is handed to a notifier.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// The subject, for mail.
    pub subject: Option<String>,
    /// The text, escaped for the notifier's medium.
    pub body: String,
    /// The notifications it was rendered from.
    pub notifications: Vec<NotificationBody>,
    /// Which of the two forms it is.
    pub form: Form,
}

/// The person as a notifier is told of them.
#[must_use]
pub fn user_ref(person: &Recipient) -> UserRef {
    UserRef {
        id: person.id,
        kind: if person.kind == "service" {
            UserKind::Service
        } else {
            UserKind::Human
        },
        username: person.username.clone(),
        display_name: person.display_name.clone(),
        avatar: Avatar {
            text: person
                .display_name
                .chars()
                .next()
                .map(|letter| letter.to_uppercase().collect())
                .unwrap_or_default(),
            color: u8::try_from(person.avatar_color).unwrap_or(0),
        },
    }
}

/// An in-app path as an address a browser opens.
#[must_use]
pub fn absolute(public_url: &str, path: &str) -> String {
    format!("{}{path}", public_url.trim_end_matches('/'))
}

/// The in-app path that opens a notification: the one the engine named in its
/// data, else its thread, else the approvals it is about, else the project or
/// topic it happened at, else the Inbox.
#[must_use]
pub fn open_path(notification: &Snapshot, places: &[PlaceRow]) -> String {
    if let Some(path) = notification
        .data
        .get("path")
        .and_then(Json::as_str)
        .filter(|path| path.starts_with('/') && !path.starts_with("//"))
    {
        return path.to_owned();
    }
    if let Some(thread) = notification.thread_id {
        return format!("/thread/{thread}");
    }
    if notification.approval_id.is_some() {
        return "/approvals".to_owned();
    }
    if let Some(node) = notification.node_id {
        match places
            .iter()
            .find(|place| place.id == node)
            .map(|place| place.kind.as_str())
        {
            Some("project") => return format!("/project/{node}"),
            Some("topic") => return format!("/topic/{node}"),
            Some("thread") => return format!("/thread/{node}"),
            _ => {}
        }
    }
    "/inbox".to_owned()
}

/// Who did it, as the notification's data names them: a name, or a person.
fn actor_of(notification: &Snapshot) -> String {
    match notification.data.get("actor") {
        Some(Json::String(name)) => name.clone(),
        Some(Json::Object(person)) => ["display_name", "name", "username"]
            .iter()
            .find_map(|key| person.get(*key).and_then(Json::as_str))
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

fn node_ref(id: Id, places: &[PlaceRow]) -> Option<NodeRef> {
    let place = places.iter().find(|place| place.id == id)?;
    let kind = match place.kind.as_str() {
        "org" => NodeKind::Org,
        "project" => NodeKind::Project,
        "topic" => NodeKind::Topic,
        "thread" => NodeKind::Thread,
        _ => return None,
    };
    Some(NodeRef {
        id,
        kind,
        name: Some(place.name.clone()),
        redacted: false,
    })
}

/// The notification as a notifier that builds its own message is given it.
fn body_of(
    notification: &Snapshot,
    index: usize,
    row_id: Id,
    rendering: &Rendering<'_>,
) -> Result<NotificationBody, RenderFault> {
    let kind: NotificationKind = serde_json::from_value(Json::String(notification.kind.clone()))
        .map_err(|_| RenderFault::UnknownKind(notification.kind.clone()))?;
    // What happens at a node and is told for nobody in particular has no row of
    // its own in anybody's Inbox: it is named by the row it is sent in.
    let id = notification.id.unwrap_or_else(|| derived_id(row_id, index));
    Ok(NotificationBody {
        id,
        kind,
        at: notification.at,
        title: notification.title.clone(),
        body: notification.body.clone(),
        url: absolute(
            rendering.public_url,
            &open_path(notification, rendering.places),
        ),
        node: notification
            .node_id
            .and_then(|node| node_ref(node, rendering.places)),
        thread_id: notification.thread_id,
        chat_id: notification.chat_id,
        data: notification.data.clone(),
        recipient: rendering.recipient.map(user_ref),
    })
}

/// An identifier for the `index`-th notification of a row that has none of its
/// own: the row's, changed in its last bytes, so that it is the same across the
/// tries.
fn derived_id(row_id: Id, index: usize) -> Id {
    let mut bytes = *row_id.as_bytes();
    let tail = u16::try_from(index).unwrap_or(u16::MAX).to_be_bytes();
    bytes[14] ^= tail[0];
    bytes[15] ^= tail[1];
    Id::from_bytes(bytes)
}

fn item_of(notification: &Snapshot, rendering: &Rendering<'_>, zone: Tz) -> Item {
    let location = notification
        .node_id
        .and_then(|node| rendering.places.iter().find(|place| place.id == node))
        .map(|place| place.path.join("/"))
        .unwrap_or_default();
    Item {
        title: notification.title.clone(),
        body: notification.body.clone(),
        url: absolute(
            rendering.public_url,
            &open_path(notification, rendering.places),
        ),
        kind: notification.kind.clone(),
        location,
        actor: actor_of(notification),
        time: notification
            .at
            .get()
            .with_timezone(&zone)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
    }
}

/// What "and so many more" says at the end of a text that was cut.
fn ending(language: &str, left: i64, inbox: &str) -> String {
    if language.to_ascii_lowercase().starts_with("zh") {
        format!("… 还有 {left} 条 — {inbox}")
    } else {
        format!("… and {left} more — {inbox}")
    }
}

/// Renders the message of a row.
///
/// `row_id` names the row, which names the notifications that have no
/// identifier of their own.
pub fn render(row_id: Id, rendering: &Rendering<'_>) -> Result<Message, RenderFault> {
    let descriptor = rendering.descriptor;
    let own: BTreeMap<String, TemplateOverride> =
        serde_json::from_value(rendering.own_texts.clone()).unwrap_or_default();
    let form = if rendering.notifications.len() == 1 {
        Form::Single
    } else {
        Form::Digest
    };
    let shipped = match descriptor.body {
        BodyKind::Text => descriptor.templates.clone(),
        BodyKind::Json => BTreeMap::new(),
    };
    let notifications = rendering
        .notifications
        .iter()
        .enumerate()
        .map(|(index, each)| body_of(each, index, row_id, rendering))
        .collect::<Result<Vec<_>, _>>()?;
    let Some(text) = resolve(&own, &shipped, rendering.language, form) else {
        return if descriptor.body == BodyKind::Json {
            Ok(Message {
                subject: None,
                body: String::new(),
                notifications,
                form,
            })
        } else {
            Err(RenderFault::NoText)
        };
    };
    let zone = rendering.recipient.map_or(Tz::UTC, |person| {
        crate::quiet_hours::zone_of(&person.timezone)
    });
    let values = Values {
        items: rendering
            .notifications
            .iter()
            .map(|each| item_of(each, rendering, zone))
            .collect(),
        recipient_name: rendering
            .recipient
            .map(|person| person.display_name.clone())
            .unwrap_or_default(),
        org_name: rendering.org_name.to_owned(),
    };
    let rendered = text.render(&values, descriptor.body)?;
    let body = match (descriptor.max_chars, descriptor.body) {
        (Some(most), BodyKind::Text) => {
            let inbox = absolute(rendering.public_url, "/inbox");
            fit(&rendered.body, most, |left| {
                ending(rendering.language, left, &inbox)
            })
            .text
        }
        _ => rendered.body,
    };
    Ok(Message {
        subject: rendered.subject,
        body,
        notifications,
        form,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_notifier_contract::descriptor::{Owner, Reaches, Target};
    use orch_notifier_contract::templates::{TemplatePair, TemplateText};
    use orch_types::new_id;
    use serde_json::json;

    fn snapshot(title: &str, body: &str, extra: Json) -> Snapshot {
        let mut value = json!({
            "id": new_id(), "kind": "turn.completed", "at": "2026-10-11T22:30:00.000Z",
            "title": title, "body": body, "data": {}
        });
        for (key, member) in extra.as_object().cloned().unwrap_or_default() {
            value[key] = member;
        }
        serde_json::from_value(value).unwrap()
    }

    fn descriptor(body: BodyKind, max_chars: Option<u32>) -> NotifierDescriptor {
        let text = |subject: Option<&str>, body: &str| TemplateText {
            subject: subject.map(str::to_owned),
            body: body.to_owned(),
        };
        let json_body = body == BodyKind::Json;
        NotifierDescriptor {
            name: "test".into(),
            title: "Test".into(),
            summary: "x".into(),
            targets: vec![Target::Person],
            fields: vec![],
            owners: vec![Owner::System],
            single: true,
            reaches: Reaches::Email,
            body,
            max_chars,
            templates: BTreeMap::from([(
                "*".to_owned(),
                TemplatePair {
                    single: text(
                        Some("[{{org.name}}] {{title}}"),
                        if json_body {
                            "{\"text\": \"{{title}}\"}"
                        } else {
                            "{{title}}\n{{body}}\n{{url}}"
                        },
                    ),
                    digest: text(
                        Some("[{{org.name}}] {{count}} unread"),
                        if json_body {
                            "{\"count\": \"{{count}}\"}"
                        } else {
                            "{{#notifications}}{{time}}  {{title}}\n{{url}}\n{{/notifications}}"
                        },
                    ),
                },
            )]),
            extensions: orch_plugin_contract::Extensions::new(),
        }
    }

    fn person() -> Recipient {
        Recipient {
            id: new_id(),
            kind: "human".into(),
            username: "ada".into(),
            display_name: "Ada".into(),
            avatar_color: 4,
            locale: "en".into(),
            timezone: "Europe/Berlin".into(),
            email: Some("ada@example.org".into()),
            email_verified_at: None,
            quiet_start_minute: None,
            quiet_end_minute: None,
            pause_until: None,
        }
    }

    fn render_with(
        descriptor: &NotifierDescriptor,
        own: &Json,
        language: &str,
        notifications: &[Snapshot],
        places: &[PlaceRow],
    ) -> Result<Message, RenderFault> {
        let person = person();
        render(
            new_id(),
            &Rendering {
                descriptor,
                own_texts: own,
                language,
                recipient: Some(&person),
                notifications,
                places,
                org_name: "Acme",
                public_url: "https://orch.example/",
            },
        )
    }

    #[test]
    fn one_notification_is_a_single_message_and_several_are_a_digest_in_the_persons_time_zone() {
        let descriptor = descriptor(BodyKind::Text, None);
        let one = render_with(
            &descriptor,
            &json!({}),
            "en",
            &[snapshot("Turn 3 finished", "all green", json!({}))],
            &[],
        )
        .unwrap();
        assert_eq!(one.form, Form::Single);
        assert_eq!(one.subject.as_deref(), Some("[Acme] Turn 3 finished"));
        assert_eq!(
            one.body,
            "Turn 3 finished\nall green\nhttps://orch.example/inbox"
        );
        let three = [
            snapshot("One", "", json!({})),
            snapshot("Two", "", json!({})),
            snapshot("Three", "", json!({})),
        ];
        let digest = render_with(&descriptor, &json!({}), "en", &three, &[]).unwrap();
        assert_eq!(digest.form, Form::Digest);
        assert_eq!(digest.subject.as_deref(), Some("[Acme] 3 unread"));
        assert_eq!(
            digest.body.matches("2026-10-12 00:30  ").count(),
            3,
            "22:30 UTC is 00:30 in Berlin: {}",
            digest.body
        );
        assert_eq!(digest.notifications.len(), 3);
    }

    #[test]
    fn a_text_of_the_channels_own_is_sent_for_its_language_then_for_every_language_before_the_shipped_one()
     {
        let descriptor = descriptor(BodyKind::Text, None);
        let own = json!({
            "*": { "single": { "subject": null, "body": "ALL: {{title}}" } },
            "zh-CN": { "single": { "subject": "ZH {{title}}", "body": "ZH: {{title}}" } }
        });
        let note = [snapshot("Hi", "", json!({}))];
        assert_eq!(
            render_with(&descriptor, &own, "zh-CN", &note, &[])
                .unwrap()
                .body,
            "ZH: Hi"
        );
        assert_eq!(
            render_with(&descriptor, &own, "en", &note, &[])
                .unwrap()
                .body,
            "ALL: Hi"
        );
        let two = [snapshot("A", "", json!({})), snapshot("B", "", json!({}))];
        assert!(
            render_with(&descriptor, &own, "en", &two, &[])
                .unwrap()
                .body
                .contains("https://orch.example/inbox"),
            "a form the channel has no text for goes with the shipped one"
        );
    }

    #[test]
    fn a_value_is_escaped_for_the_medium_and_a_json_notifier_is_sent_only_a_text_of_its_own() {
        let descriptor = descriptor(BodyKind::Json, None);
        let note = [snapshot("Say \"hi\"\nnow", "", json!({}))];
        let without = render_with(&descriptor, &json!({}), "en", &note, &[]).unwrap();
        assert_eq!(
            without.body, "",
            "the shipped text of a JSON notifier is an example, not a default"
        );
        assert_eq!(without.subject, None);
        assert_eq!(
            without.notifications.len(),
            1,
            "the notifier builds its body from these"
        );
        let own =
            json!({ "*": { "single": { "subject": null, "body": "{\"text\": \"{{title}}\"}" } } });
        let with = render_with(&descriptor, &own, "en", &note, &[]).unwrap();
        let parsed: Json = serde_json::from_str(&with.body).unwrap();
        assert_eq!(parsed["text"], "Say \"hi\"\nnow");
    }

    #[test]
    fn a_text_longer_than_a_message_takes_is_cut_at_a_line_and_ends_with_how_many_more_and_the_inbox()
     {
        let descriptor = descriptor(BodyKind::Text, Some(120));
        let many: Vec<Snapshot> = (1..=12)
            .map(|n| snapshot(&format!("Notification number {n}"), "", json!({})))
            .collect();
        let cut = render_with(&descriptor, &json!({}), "en", &many, &[]).unwrap();
        assert!(cut.body.chars().count() <= 120, "{}", cut.body);
        assert!(
            cut.body.contains("more — https://orch.example/inbox"),
            "{}",
            cut.body
        );
        let chinese = render_with(&descriptor, &json!({}), "zh-CN", &many, &[]).unwrap();
        assert!(chinese.body.contains("还有"), "{}", chinese.body);
        assert_eq!(
            cut.notifications.len(),
            12,
            "what the notifier is given beside the text is not cut"
        );
    }

    #[test]
    fn where_a_notification_opens_is_what_the_engine_named_or_its_thread_or_its_place_or_the_inbox()
    {
        let place = |kind: &str| PlaceRow {
            id: new_id(),
            kind: kind.into(),
            name: "Fix login".into(),
            path: vec!["Acme".into(), "Web".into(), "Fix login".into()],
            org_id: new_id(),
            org_name: "Acme".into(),
        };
        let (project, topic) = (place("project"), place("topic"));
        let places = [project.clone(), topic.clone()];
        let at = |extra: Json| open_path(&snapshot("x", "", extra), &places);
        assert_eq!(at(json!({ "data": { "path": "/doc/42" } })), "/doc/42");
        assert_eq!(
            at(json!({ "data": { "path": "//evil.example" } })),
            "/inbox",
            "a path stays in the application"
        );
        let thread = new_id();
        assert_eq!(
            at(json!({ "thread_id": thread, "node_id": project.id })),
            format!("/thread/{thread}")
        );
        assert_eq!(at(json!({ "approval_id": new_id() })), "/approvals");
        assert_eq!(
            at(json!({ "node_id": project.id })),
            format!("/project/{}", project.id)
        );
        assert_eq!(
            at(json!({ "node_id": topic.id })),
            format!("/topic/{}", topic.id)
        );
        assert_eq!(at(json!({})), "/inbox");
        let shown = render_with(
            &descriptor(BodyKind::Text, None),
            &json!({}),
            "en",
            &[snapshot("x", "", json!({ "node_id": topic.id }))],
            &places,
        )
        .unwrap();
        assert_eq!(
            shown.notifications[0]
                .node
                .as_ref()
                .unwrap()
                .name
                .as_deref(),
            Some("Fix login")
        );
    }

    #[test]
    fn a_notification_without_an_identifier_of_its_own_is_named_by_its_row_the_same_way_each_try() {
        let row = new_id();
        assert_eq!(derived_id(row, 0), row);
        assert_ne!(derived_id(row, 1), row);
        assert_eq!(derived_id(row, 1), derived_id(row, 1));
        assert_ne!(derived_id(row, 1), derived_id(row, 2));
    }

    #[test]
    fn a_kind_that_is_not_one_is_refused_by_name() {
        let mut odd = snapshot("x", "", json!({}));
        odd.kind = "no.such.kind".into();
        assert_eq!(
            render_with(
                &descriptor(BodyKind::Text, None),
                &json!({}),
                "en",
                &[odd],
                &[]
            )
            .unwrap_err(),
            RenderFault::UnknownKind("no.such.kind".into())
        );
    }
}
