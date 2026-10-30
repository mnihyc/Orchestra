//! A kind's identity; whom it reaches (a person's address, their devices, a chat linked by a code, a place somebody configured); who may set one up and how many; the fields a channel or a destination is made from; and the texts it ships.
//!
//! A notifier is the kind of a **channel**, which reaches a person through that person's route, or of a
//! **destination**, which a node is told at; `webhook` is both. A notifier whose `targets` hold `person`
//! is offered wherever its `owners` set channels up, and one whose `targets` hold `destination` wherever
//! destinations are added, each with a form made from its `fields`. So an integration with a chat tool is
//! one more notifier: its fields, the texts it ships, and its way of linking people where it has to.
//!
//! How a channel finds one person is its `reaches`, and it is what a person's route needs before it can be
//! on:
//!
//! | `reaches` | The route's address | Needs | Built in |
//! |-----------|---------------------|-------|----------|
//! | `email` | The person's confirmed address, which no route stores | A confirmed address | `email` |
//! | `devices` | Each of the person's push subscriptions; one delivery for each | A browser that took push | `push` |
//! | `link` | What the notifier learned when the person followed its link: for a bot, the chat it may write to | The link followed | `telegram` |
//! | `channel` | None: the address is the channel's own, and the body names the person | Nothing | `webhook` |

use std::collections::{BTreeMap, BTreeSet};

use orch_plugin_contract::Extensions;
use orch_types::Field;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::templates::{BodyKind, Form, TemplateError, TemplatePair};

/// What one of a notifier can be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// A channel, which reaches a person through their route.
    Person,
    /// A destination, which a node is told at.
    Destination,
}

/// Who may set up a channel of a notifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    /// A system admin, for everybody.
    System,
    /// A root org, for its members.
    Org,
    /// A person, for themself.
    User,
}

/// How a channel finds one person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reaches {
    /// By the person's confirmed address.
    Email,
    /// By each of the person's devices.
    Devices,
    /// By what the notifier learned when the person followed its link.
    Link,
    /// By nothing of the person's: the address is the channel's own and the body names the person.
    Channel,
}

/// What a person's route lacks before it can be switched on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Needs {
    /// A confirmed address.
    VerifiedEmail,
    /// A browser that took push.
    Device,
    /// The link followed.
    Link,
}

impl Reaches {
    /// What a route through such a channel needs before it can be on; `None` for a channel that needs
    /// nothing of the person.
    #[must_use]
    pub const fn needs(self) -> Option<Needs> {
        match self {
            Self::Email => Some(Needs::VerifiedEmail),
            Self::Devices => Some(Needs::Device),
            Self::Link => Some(Needs::Link),
            Self::Channel => None,
        }
    }

    /// Whether a person is reached once for each of their devices: one delivery for each.
    #[must_use]
    pub const fn one_delivery_per_device(self) -> bool {
        matches!(self, Self::Devices)
    }

    /// Whether the route stores an address of its own: what the notifier learned when the link was followed.
    #[must_use]
    pub const fn stores_an_address(self) -> bool {
        matches!(self, Self::Link)
    }

    /// Whether the notifier has to make a link and ask who followed it (`link_url`, `poll_links`).
    #[must_use]
    pub const fn links_people(self) -> bool {
        matches!(self, Self::Link)
    }
}

/// Everything a notifier declares about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NotifierDescriptor {
    /// Its name, which a channel names as its kind.
    pub name: String,
    /// What it is called.
    pub title: String,
    /// What it does, in a sentence.
    pub summary: String,
    /// What one of it can be: a channel, a destination, or both.
    pub targets: Vec<Target>,
    /// What one of it is made from.
    #[serde(default)]
    pub fields: Vec<Field>,
    /// For a channel: who may set one up. Empty for a notifier that is only a destination.
    #[serde(default)]
    pub owners: Vec<Owner>,
    /// For a channel: the system and each root org have at most one of it, such as a mail server or a bot.
    #[serde(default)]
    pub single: bool,
    /// For a channel: how it finds one person.
    pub reaches: Reaches,
    /// What it sends, and so how values are escaped.
    pub body: BodyKind,
    /// The longest body one message of it takes, in characters.
    #[serde(default)]
    pub max_chars: Option<u32>,
    /// The texts it ships, by language tag, `*` serving every language without its own.
    #[serde(default)]
    pub templates: BTreeMap<String, TemplatePair>,
    /// Members that belong to the implementation, kept and never interpreted.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// What is wrong with a notifier's declaration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DescriptorFault {
    /// It has no name.
    #[error("a notifier has a name")]
    NoName,
    /// It can be neither a channel nor a destination.
    #[error("a notifier can be a channel, a destination, or both")]
    NoTarget,
    /// A channel that nobody may set up.
    #[error("a notifier that can be a channel says who may set one up")]
    ChannelWithoutOwner,
    /// Owners for a notifier that is not a channel.
    #[error("only a channel has owners and is single")]
    OwnersWithoutChannel,
    /// Two fields have one name.
    #[error("the field {0:?} is declared twice")]
    DuplicateField(String),
    /// `max_chars` is zero.
    #[error("max_chars is at least 1, or left out")]
    MaxCharsZero,
    /// The notifier ships no text for every language: a language it did not list would send nothing.
    #[error("a notifier ships texts for every language (\"*\")")]
    NoDefaultTexts,
    /// A shipped text does not pass the check a saved one does.
    #[error("the shipped {form:?} text for {language:?}: {error}")]
    Text {
        /// The language key.
        language: String,
        /// The form.
        form: Form,
        /// What is wrong.
        error: TemplateError,
    },
}

impl NotifierDescriptor {
    /// Whether one of it can be a channel.
    #[must_use]
    pub fn is_channel(&self) -> bool {
        self.targets.contains(&Target::Person)
    }

    /// Whether one of it can be a destination.
    #[must_use]
    pub fn is_destination(&self) -> bool {
        self.targets.contains(&Target::Destination)
    }

    /// Whether it is offered where `owner` sets channels up: it can be a channel and `owner` may set one up.
    #[must_use]
    pub fn offered_to(&self, owner: Owner) -> bool {
        self.is_channel() && self.owners.contains(&owner)
    }

    /// Whether the system and each root org have at most one of it.
    #[must_use]
    pub fn is_single(&self) -> bool {
        self.is_channel() && self.single
    }

    /// Checks that the declaration is consistent with itself, shipped texts included. Every fault is
    /// returned.
    pub fn check(&self) -> Result<(), Vec<DescriptorFault>> {
        let mut faults = Vec::new();
        if self.name.is_empty() {
            faults.push(DescriptorFault::NoName);
        }
        if self.targets.is_empty() {
            faults.push(DescriptorFault::NoTarget);
        }
        if self.is_channel() && self.owners.is_empty() {
            faults.push(DescriptorFault::ChannelWithoutOwner);
        }
        if !self.is_channel() && (!self.owners.is_empty() || self.single) {
            faults.push(DescriptorFault::OwnersWithoutChannel);
        }
        let mut seen = BTreeSet::new();
        for field in &self.fields {
            if !seen.insert(field.name.as_str()) {
                faults.push(DescriptorFault::DuplicateField(field.name.clone()));
            }
        }
        if self.max_chars == Some(0) {
            faults.push(DescriptorFault::MaxCharsZero);
        }
        if !self.templates.contains_key(crate::templates::ANY_LANGUAGE) {
            faults.push(DescriptorFault::NoDefaultTexts);
        }
        for (language, pair) in &self.templates {
            for form in [Form::Single, Form::Digest] {
                if let Err(error) = pair.get(form).check(self.body) {
                    faults.push(DescriptorFault::Text {
                        language: language.clone(),
                        form,
                        error,
                    });
                }
            }
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(subject: Option<&str>, body: &str) -> serde_json::Value {
        json!({ "subject": subject, "body": body })
    }

    /// A bot that links people by a code, like `telegram`.
    fn telegram() -> serde_json::Value {
        json!({
            "name": "telegram", "title": "Telegram", "summary": "Messages from a bot to the people who started a chat with it.",
            "targets": ["person"],
            "fields": [ { "name": "bot_token", "label": "Bot token", "type": "secret", "required": true, "help": null,
                          "choices": null, "default": null, "max_bytes": null } ],
            "owners": ["system", "org"], "single": true, "reaches": "link", "body": "text", "max_chars": 4096,
            "templates": { "*": {
                "single": text(None, "{{title}}\n{{body}}\n{{url}}"),
                "digest": text(None, "{{#notifications}}{{time}}  {{title}}\n{{url}}\n{{/notifications}}") } }
        })
    }

    /// A webhook, which is both a channel and a destination and whose body is JSON.
    fn webhook() -> serde_json::Value {
        json!({
            "name": "webhook", "title": "Webhook", "summary": "Posts what happened to an address.",
            "targets": ["person", "destination"],
            "fields": [ { "name": "url", "label": "URL", "type": "text", "required": true, "help": null,
                          "choices": null, "default": null, "max_bytes": null },
                        { "name": "secret", "label": "Secret", "type": "secret", "required": false, "help": null,
                          "choices": null, "default": null, "max_bytes": null } ],
            "owners": ["system", "org", "user"], "single": false, "reaches": "channel", "body": "json",
            "templates": { "*": {
                "single": text(None, "{\"text\": \"{{title}}: {{body}} {{url}}\"}"),
                "digest": text(None, "{\"text\": \"{{count}} unread\", \"items\": [{{#notifications}}\"{{title}}\",{{/notifications}} null]}") } }
        })
    }

    #[test]
    fn two_of_the_built_in_notifiers_read_and_write_as_their_declarations_say() {
        for written in [telegram(), webhook()] {
            let read: NotifierDescriptor = serde_json::from_value(written.clone()).unwrap();
            assert_eq!(read.check(), Ok(()), "{}", read.name);
            let back = serde_json::to_value(&read).unwrap();
            let mut want = written;
            want.as_object_mut()
                .unwrap()
                .entry("max_chars")
                .or_insert(json!(null));
            assert_eq!(back, want);
        }
    }

    #[test]
    fn what_a_notifier_can_be_decides_where_it_is_offered() {
        let bot: NotifierDescriptor = serde_json::from_value(telegram()).unwrap();
        assert!(bot.is_channel() && !bot.is_destination() && bot.is_single());
        assert!(
            bot.offered_to(Owner::System)
                && bot.offered_to(Owner::Org)
                && !bot.offered_to(Owner::User)
        );
        let hook: NotifierDescriptor = serde_json::from_value(webhook()).unwrap();
        assert!(hook.is_channel() && hook.is_destination() && !hook.is_single());
        assert!(hook.offered_to(Owner::User), "a person's own are webhooks");
        let mut room = hook.clone();
        room.targets = vec![Target::Destination];
        room.owners.clear();
        assert!(!room.is_channel() && room.is_destination());
        assert!(
            !room.offered_to(Owner::System),
            "a destination's notifier is offered where destinations are added"
        );
        assert_eq!(room.check(), Ok(()));
    }

    #[test]
    fn how_a_channel_finds_a_person_is_what_their_route_needs() {
        assert_eq!(Reaches::Email.needs(), Some(Needs::VerifiedEmail));
        assert_eq!(Reaches::Devices.needs(), Some(Needs::Device));
        assert_eq!(Reaches::Link.needs(), Some(Needs::Link));
        assert_eq!(Reaches::Channel.needs(), None);
        assert!(
            Reaches::Devices.one_delivery_per_device() && !Reaches::Email.one_delivery_per_device()
        );
        assert!(Reaches::Link.stores_an_address() && Reaches::Link.links_people());
        assert!(!Reaches::Channel.stores_an_address() && !Reaches::Email.links_people());
        assert_eq!(
            serde_json::to_value(Needs::VerifiedEmail).unwrap(),
            json!("verified_email")
        );
    }

    #[test]
    fn a_declaration_that_cannot_hold_together_is_refused() {
        let mut broken: NotifierDescriptor = serde_json::from_value(telegram()).unwrap();
        broken.name.clear();
        broken.owners.clear();
        broken.max_chars = Some(0);
        broken.fields.push(broken.fields[0].clone());
        broken.templates.clear();
        let faults = broken.check().unwrap_err();
        for expected in [
            DescriptorFault::NoName,
            DescriptorFault::ChannelWithoutOwner,
            DescriptorFault::MaxCharsZero,
            DescriptorFault::DuplicateField("bot_token".into()),
            DescriptorFault::NoDefaultTexts,
        ] {
            assert!(faults.contains(&expected), "{expected:?} in {faults:?}");
        }
        let mut no_target = broken.clone();
        no_target.targets.clear();
        assert!(
            no_target
                .check()
                .unwrap_err()
                .contains(&DescriptorFault::NoTarget)
        );
        let mut owners_for_a_room = webhook_descriptor();
        owners_for_a_room.targets = vec![Target::Destination];
        assert!(
            owners_for_a_room
                .check()
                .unwrap_err()
                .contains(&DescriptorFault::OwnersWithoutChannel)
        );
    }

    fn webhook_descriptor() -> NotifierDescriptor {
        serde_json::from_value(webhook()).unwrap()
    }

    #[test]
    fn a_shipped_text_is_held_to_the_check_a_saved_one_passes() {
        let mut bad = webhook_descriptor();
        bad.templates.get_mut("*").unwrap().single.body = "{{title}} finished".into();
        let faults = bad.check().unwrap_err();
        assert!(
            matches!(
                faults.as_slice(),
                [DescriptorFault::Text {
                    form: Form::Single,
                    error: TemplateError::NotJson(_),
                    ..
                }]
            ),
            "{faults:?}"
        );
        let mut unknown = webhook_descriptor();
        unknown.templates.get_mut("*").unwrap().digest.body = "{{nope}}".into();
        assert!(matches!(
            unknown.check().unwrap_err().as_slice(),
            [DescriptorFault::Text {
                form: Form::Digest,
                error: TemplateError::UnknownPlaceholder(_),
                ..
            }]
        ));
    }

    #[test]
    fn members_of_the_implementations_own_are_kept() {
        let mut with = telegram();
        with["x_api"] = json!("https://api.telegram.org");
        let read: NotifierDescriptor = serde_json::from_value(with).unwrap();
        assert_eq!(
            read.extensions.get("x_api"),
            Some(&json!("https://api.telegram.org"))
        );
    }
}
