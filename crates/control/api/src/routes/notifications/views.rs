//! The shapes the requests answer with: a channel together with the caller's own route through it, a route, a destination, a kind of notifier in the caller's language, and the texts a channel keeps.
//!
//! What a person may read of a channel depends on whether they may change it:
//! its fields and the names of its secrets are for whoever looks after it, and
//! so is the reason its last delivery failed, which says things about a server
//! that a member has no use for. Nobody is ever shown the value of a secret.

use orch_api_contract::nodes::NodeRef;
use orch_api_contract::notifications::{
    Destination, DestinationLevel, NotificationChannel, NotificationChannelOwner, NotificationKind,
    NotificationRoute, NotificationRouteChannel, NotificationRouteFailure, NotificationRouteHealth,
    NotificationRouteLink, NotificationRouteNeeds, Notifier, NotifierOwner, NotifierTarget,
    NotifierTemplates, TemplateText, TemplatesByLanguage,
};
use orch_db::queries::notifications::channels::{ChannelRow, DestinationRow};
use orch_db::queries::notifications::routes::RouteRow;
use orch_notifier_contract::descriptor::{Needs, NotifierDescriptor, Owner, Target};
use orch_notifier_contract::templates::{self, Form, TemplateOverride};
use orch_types::{Id, Json, Time};
use std::collections::BTreeMap;

/// The member of a route's address that names whoever followed its link.
const LABEL: &str = "label";

/// What a route lacks, as the contract names it.
#[must_use]
pub fn needs_view(needs: Needs) -> NotificationRouteNeeds {
    match needs {
        Needs::VerifiedEmail => NotificationRouteNeeds::VerifiedEmail,
        Needs::Device => NotificationRouteNeeds::Device,
        Needs::Link => NotificationRouteNeeds::Link,
    }
}

/// The kind of notification a word names; none for a word that is none.
#[must_use]
pub fn kind_of(word: &str) -> Option<NotificationKind> {
    serde_json::from_value(Json::String(word.to_owned())).ok()
}

/// What is known of a person's route for the view.
#[derive(Debug, Clone)]
pub struct RouteFacts<'a> {
    /// The channel where it still exists; none once it was removed.
    pub channel_id: Option<Id>,
    /// The channel as the route shows it.
    pub channel: Option<NotificationRouteChannel>,
    /// The route, where it was made.
    pub route: Option<&'a RouteRow>,
    /// What it lacks before it can be on.
    pub needs: Option<Needs>,
    /// How many unread notifications wait in its scope.
    pub waiting_count: i64,
    /// A bot's link, while it waits to be pressed, for the route's own person.
    pub link: Option<NotificationRouteLink>,
}

/// How a route fared, as the contract names it; `never` for a word it does not know.
#[must_use]
pub fn health_view(word: &str) -> NotificationRouteHealth {
    serde_json::from_value(Json::String(word.to_owned())).unwrap_or(NotificationRouteHealth::Never)
}

/// Why a route fails, while it is in a failure episode.
#[must_use]
pub fn failure_view(route: &RouteRow) -> Option<NotificationRouteFailure> {
    route.failure_episode?;
    let kept = route.last_error.as_ref()?;
    Some(NotificationRouteFailure {
        code: kept
            .get("code")
            .and_then(Json::as_str)
            .unwrap_or("failed")
            .to_owned(),
        message: kept
            .get("message")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned(),
        affected_devices: kept
            .get("affected_devices")
            .and_then(Json::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

/// A channel that exists, as a route shows it.
#[must_use]
pub fn route_channel_of(
    row: &ChannelRow,
    orgs: &BTreeMap<Id, NodeRef>,
) -> NotificationRouteChannel {
    NotificationRouteChannel {
        id: row.id,
        kind: row.kind.clone(),
        name: row.name.clone(),
        org: row.org_id.and_then(|org| orgs.get(&org).cloned()),
    }
}

/// The channel a route keeps, also after it was removed; its org only where the caller may
/// still name it.
#[must_use]
pub fn route_channel_kept(
    route: &RouteRow,
    orgs: &BTreeMap<Id, NodeRef>,
) -> Option<NotificationRouteChannel> {
    let id = route.kept_channel_id()?;
    let org: Option<Id> = route
        .kept_channel_text("org_id")
        .and_then(|text| text.parse().ok());
    Some(NotificationRouteChannel {
        id,
        kind: route
            .kept_channel_text("kind")
            .unwrap_or_default()
            .to_owned(),
        name: route
            .kept_channel_text("name")
            .unwrap_or_default()
            .to_owned(),
        org: org.and_then(|org| orgs.get(&org).cloned()),
    })
}

/// A route as the caller sees it. A person who never changed theirs has none
/// stored, and sees what every route starts as: off, for everything, at once.
#[must_use]
pub fn route_view(facts: RouteFacts<'_>) -> NotificationRoute {
    let RouteFacts {
        channel_id,
        channel,
        route,
        needs,
        waiting_count,
        link,
    } = facts;
    let minutes = |value: Option<i32>| value.and_then(|minutes| u32::try_from(minutes).ok());
    NotificationRoute {
        id: route.map(|route| route.id),
        channel_id,
        channel,
        enabled: route.is_some_and(|route| route.enabled),
        health: route.map_or(NotificationRouteHealth::Never, |route| {
            health_view(&route.health)
        }),
        last_ok_at: route.and_then(|route| route.last_ok_at).map(Time::new),
        failed_at: route
            .filter(|route| route.failure_episode.is_some())
            .and_then(|route| route.failed_at)
            .map(Time::new),
        failure: route.and_then(failure_view),
        waiting_count,
        needs: needs.map(needs_view),
        link,
        label: route
            .and_then(|route| route.address.get(LABEL))
            .and_then(Json::as_str)
            .map(str::to_owned),
        kinds: route
            .map(|route| {
                route
                    .kinds
                    .iter()
                    .filter_map(|word| kind_of(word))
                    .collect()
            })
            .unwrap_or_default(),
        unread_delay_minutes: route.and_then(|route| minutes(route.unread_delay_minutes)),
        min_interval_minutes: route.and_then(|route| minutes(route.min_interval_minutes)),
        last_sent_at: route.and_then(|route| route.last_sent_at).map(Time::new),
    }
}

/// The texts a channel or a destination keeps, as the contract has them.
#[must_use]
pub fn templates_view(stored: &Json) -> TemplatesByLanguage {
    serde_json::from_value(stored.clone()).unwrap_or_default()
}

/// The texts as the notifier contract checks them.
pub fn overrides_of(
    given: &TemplatesByLanguage,
) -> Result<BTreeMap<String, TemplateOverride>, serde_json::Error> {
    serde_json::from_value(serde_json::to_value(given)?)
}

/// Whose a channel is, with the name of its org where it is one's.
#[must_use]
pub fn owner_view(row: &ChannelRow, orgs: &BTreeMap<Id, NodeRef>) -> NotificationChannelOwner {
    match (row.org_id, row.user_id) {
        (Some(org), _) => orgs.get(&org).cloned().map_or(
            // An org the caller cannot name is still the owner; it is shown
            // by its identifier alone.
            NotificationChannelOwner::Org(NodeRef {
                thread_display: None,
                id: org,
                kind: orch_api_contract::nodes::NodeKind::Org,
                name: None,
                redacted: true,
            }),
            NotificationChannelOwner::Org,
        ),
        (None, Some(_)) => NotificationChannelOwner::Me,
        (None, None) => NotificationChannelOwner::System,
    }
}

/// What is known of a channel for the caller.
#[derive(Debug, Clone)]
pub struct ChannelFacts<'a> {
    /// The channel.
    pub row: &'a ChannelRow,
    /// Whose it is.
    pub owner: NotificationChannelOwner,
    /// Whether the caller may change it.
    pub can_edit: bool,
    /// The names of its secrets that have a value.
    pub secrets_set: Vec<String>,
    /// How many people have a route through it on; shown to whoever may change it alone.
    pub enabled_route_count: i64,
    /// The caller's route through it.
    pub route: NotificationRoute,
}

/// A channel as the caller reads it.
#[must_use]
pub fn channel_view(facts: ChannelFacts<'_>) -> NotificationChannel {
    let ChannelFacts {
        row,
        owner,
        can_edit,
        secrets_set,
        enabled_route_count,
        route,
    } = facts;
    NotificationChannel {
        id: row.id,
        kind: row.kind.clone(),
        name: row.name.clone(),
        owner,
        config: if can_edit {
            row.config.clone()
        } else {
            Json::Object(serde_json::Map::new())
        },
        secrets_set: if can_edit { secrets_set } else { Vec::new() },
        templates: templates_view(&row.templates),
        enabled: row.enabled,
        last_ok_at: row.last_ok_at.map(Time::new),
        last_error: if can_edit {
            row.last_error.clone()
        } else {
            None
        },
        can_edit,
        enabled_route_count: can_edit.then_some(enabled_route_count),
        route,
    }
}

/// How much of what happens a destination is sent.
#[must_use]
pub fn level_of(word: &str) -> DestinationLevel {
    if word == "all" {
        DestinationLevel::All
    } else {
        DestinationLevel::Important
    }
}

/// The word the schema keeps for a destination's level.
#[must_use]
pub const fn level_word(level: DestinationLevel) -> &'static str {
    match level {
        DestinationLevel::All => "all",
        DestinationLevel::Important => "important",
    }
}

/// A destination as whoever manages its node reads it.
#[must_use]
pub fn destination_view(
    row: &DestinationRow,
    node: NodeRef,
    secrets_set: Vec<String>,
) -> Destination {
    Destination {
        id: row.id,
        channel: row.channel.clone(),
        name: row.name.clone(),
        node,
        config: row.config.clone(),
        secrets_set,
        templates: templates_view(&row.templates),
        kinds: row.kinds.clone(),
        level: level_of(&row.level),
        enabled: row.enabled,
        last_ok_at: row.last_ok_at.map(Time::new),
        last_error: row.last_error.clone(),
        created_at: Time::new(row.created_at),
    }
}

/// A kind of notifier, with the texts it ships in the caller's language.
#[must_use]
pub fn notifier_view(descriptor: &NotifierDescriptor, language: &str) -> Notifier {
    let shipped = |form: Form| {
        templates::resolve(&BTreeMap::new(), &descriptor.templates, language, form)
            .map(|text| TemplateText {
                subject: text.subject.clone(),
                body: text.body.clone(),
            })
            .unwrap_or(TemplateText {
                subject: None,
                body: String::new(),
            })
    };
    Notifier {
        name: descriptor.name.clone(),
        title: descriptor.title.clone(),
        summary: descriptor.summary.clone(),
        targets: descriptor
            .targets
            .iter()
            .map(|target| match target {
                Target::Person => NotifierTarget::Person,
                Target::Destination => NotifierTarget::Destination,
            })
            .collect(),
        fields: descriptor.fields.clone(),
        owners: descriptor
            .owners
            .iter()
            .map(|owner| match owner {
                Owner::System => NotifierOwner::System,
                Owner::Org => NotifierOwner::Org,
                Owner::User => NotifierOwner::User,
            })
            .collect(),
        single: descriptor.single,
        links: descriptor.reaches.links_people(),
        templates: NotifierTemplates {
            single: shipped(Form::Single),
            digest: shipped(Form::Digest),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(templates: Json) -> ChannelRow {
        ChannelRow {
            id: orch_types::new_id(),
            org_id: None,
            user_id: None,
            kind: "webhook".to_owned(),
            name: "Hook".to_owned(),
            config: json!({ "url": "https://hooks.example/x" }),
            templates,
            enabled: true,
            link_cursor: None,
            last_ok_at: None,
            last_error: Some("the server said no".to_owned()),
            created_by: orch_types::new_id(),
            created_at: chrono::DateTime::UNIX_EPOCH,
            updated_at: chrono::DateTime::UNIX_EPOCH,
        }
    }

    fn unmade(channel: Option<Id>, needs: Option<Needs>) -> NotificationRoute {
        route_view(RouteFacts {
            channel_id: channel,
            channel: None,
            route: None,
            needs,
            waiting_count: 0,
            link: None,
        })
    }

    #[test]
    fn a_member_sees_a_channel_without_its_fields_its_secrets_its_failures_and_its_count() {
        let channel = row(json!({}));
        let route = unmade(Some(channel.id), None);
        let seen_by_member = channel_view(ChannelFacts {
            row: &channel,
            owner: NotificationChannelOwner::System,
            can_edit: false,
            secrets_set: vec!["secret".to_owned()],
            enabled_route_count: 7,
            route: route.clone(),
        });
        assert_eq!(seen_by_member.config, json!({}));
        assert!(seen_by_member.secrets_set.is_empty());
        assert_eq!(seen_by_member.last_error, None);
        assert_eq!(seen_by_member.enabled_route_count, None);
        let seen_by_admin = channel_view(ChannelFacts {
            row: &channel,
            owner: NotificationChannelOwner::System,
            can_edit: true,
            secrets_set: vec!["secret".to_owned()],
            enabled_route_count: 7,
            route,
        });
        assert_eq!(seen_by_admin.config["url"], "https://hooks.example/x");
        assert_eq!(seen_by_admin.secrets_set, ["secret"]);
        assert_eq!(
            seen_by_admin.last_error.as_deref(),
            Some("the server said no")
        );
        assert_eq!(seen_by_admin.enabled_route_count, Some(7));
    }

    #[test]
    fn a_route_nobody_changed_is_off_for_everything_and_at_once() {
        let channel = orch_types::new_id();
        let shown = unmade(Some(channel), Some(Needs::Link));
        assert!(!shown.enabled);
        assert_eq!(shown.id, None);
        assert_eq!(shown.health, NotificationRouteHealth::Never);
        assert_eq!(shown.needs, Some(NotificationRouteNeeds::Link));
        assert!(shown.kinds.is_empty());
        assert_eq!(
            (shown.unread_delay_minutes, shown.min_interval_minutes),
            (None, None)
        );
    }

    #[test]
    fn the_texts_of_a_channel_are_read_back_as_they_were_kept() {
        let kept = json!({ "*": { "single": { "subject": null, "body": "{{title}}" } } });
        let shown = templates_view(&kept);
        assert_eq!(
            shown["*"].single.as_ref().map(|text| text.body.as_str()),
            Some("{{title}}")
        );
        assert!(shown["*"].digest.is_none());
        let back = overrides_of(&shown).unwrap();
        assert_eq!(
            back["*"].single.as_ref().map(|text| text.body.as_str()),
            Some("{{title}}")
        );
    }
}
