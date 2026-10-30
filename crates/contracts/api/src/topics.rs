//! Topic sync, announcements with their category and whom they went to, and the environments a topic's threads use.
//!
//! A [`TopicSync`] brings a client up to date on a topic: the topic, its
//! threads, the jobs about them and the latest announcements. An announcement
//! is news that a person, or the agent of one thread, sends to the threads of
//! a topic, or of every open topic of a project; how it reaches a thread that
//! is at work is set by its delivery, and what it did is counted in
//! [`AnnouncementDelivered`]. The environments of a topic are listed beside
//! the places where one more could be made.

use orch_types::{Id, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::Page;
use crate::environment::{Environment, Place, PlaceListQuery};
use crate::nodes::Topic;
use crate::routes::Access::{Permission, Rule};
use crate::threads::{AnnouncementCategory, AnnouncementFrom, AnnouncementSeverity, Thread};
use crate::workspace::Job;

/// Which threads of a topic an announcement went to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementAudience {
    /// All of them.
    All,
    /// Those that are at work.
    Active,
    /// None: only the followers of its thread hear of it, as of a milestone unless it is raised.
    #[serde(rename = "none")]
    Nobody,
}

/// An announcement sent to one thread: a message sent by another thread's agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AnnouncementThreadTarget {
    /// The thread.
    pub thread_id: Id,
}

/// Whom an announcement went to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum AnnouncementTargets {
    /// Some or all of a topic's threads, or only the followers of the sending thread.
    Audience(AnnouncementAudience),
    /// One thread. Its title is empty.
    Thread(AnnouncementThreadTarget),
}

/// How an announcement reaches a thread that is at work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementDelivery {
    /// It interrupts the turn.
    Interrupt,
    /// It is delivered into the turn.
    Steer,
    /// It waits for the next turn.
    NextTurn,
}

/// What an announcement does where it may not reach into a running turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementFallback {
    /// It waits for the next turn.
    NextTurn,
    /// It interrupts the turn.
    Interrupt,
}

/// A file that a message between threads carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnnouncementFile {
    /// Its name.
    pub name: String,
    /// Its size in bytes.
    pub size: i64,
}

/// How many threads an announcement reached in each way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AnnouncementDelivered {
    /// Threads whose turn it interrupted.
    pub interrupted: i64,
    /// Threads whose turn it was delivered into.
    pub steered: i64,
    /// Threads where it was queued for the next turn.
    pub queued: i64,
    /// Threads where it was put off.
    pub deferred: i64,
}

/// News sent to the threads of a topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Announcement {
    /// Its identifier.
    pub id: Id,
    /// The topic.
    pub topic_id: Id,
    /// The shared identifier of one announcement to a place, including an org or a project.
    pub group_id: Option<Id>,
    /// What kind of news it is.
    pub category: AnnouncementCategory,
    /// Its title; empty for a message sent to one thread.
    pub title: String,
    /// Its text.
    pub body: String,
    /// How much it matters.
    pub severity: AnnouncementSeverity,
    /// Who sent it.
    pub from: AnnouncementFrom,
    /// Whom it went to.
    pub targets: AnnouncementTargets,
    /// What such a message carries.
    pub files: Vec<AnnouncementFile>,
    /// How it reaches a thread that is at work.
    pub delivery: AnnouncementDelivery,
    /// What it does where it may not reach into a running turn.
    pub fallback: AnnouncementFallback,
    /// Whether it wakes a thread that is idle, as applied.
    pub wake_idle: bool,
    /// What it did.
    pub delivered: AnnouncementDelivered,
    /// When it was sent.
    pub created_at: Time,
    /// Its place in the topic's feed.
    pub topic_seq: Seq,
}

/// What brings a client up to date on a topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TopicSync {
    /// The topic.
    pub topic: Topic,
    /// The sequence of its feed now.
    pub seq: Seq,
    /// The sequence the request started from.
    pub since: Option<Seq>,
    /// False when too much changed for the answer to hold it, and the client reads again from
    /// the start.
    pub complete: bool,
    /// Its threads: all on a first load, and the changed ones from a sequence.
    pub threads: Vec<Thread>,
    /// On a first load, the active jobs about its threads.
    pub jobs: Vec<Job>,
    /// On a first load, the last twenty announcements. An environment has a feed of its own.
    pub announcements: Vec<Announcement>,
    /// Readable schedule metadata; the authored definition is returned to its author only.
    #[serde(default)]
    pub input_schedules: Vec<crate::input_schedules::InputSchedule>,
}

/// What a topic's feed is read from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopicSyncGetQuery {
    /// Only what changed after this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Seq>,
}

/// Names the environment that new threads of a topic join, or none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TopicDefaultEnvironmentSetBody {
    /// The environment; `null` clears this place's choice and inherits the place above.
    pub environment_id: Option<Id>,
}

/// Which announcements of a topic are listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementTo {
    /// Those that went to the topic's threads.
    Threads,
    /// Also the milestones that only followers heard of and the messages between two threads, each
    /// for those who may view its thread.
    All,
}

/// The announcements of a topic, newest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeAnnouncementListQuery {
    /// Only this category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<AnnouncementCategory>,
    /// Which to list; `threads` when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<AnnouncementTo>,
    /// How many entries to return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Which threads an announcement is sent to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnouncementSendAudience {
    /// All the threads.
    All,
    /// Those that are at work.
    Active,
}

/// Sends an announcement. A member left out takes the resolved `broadcast.*` setting. `steer` and
/// `interrupt`, also as the fallback, reach into a running turn only on the threads where the
/// sender holds `thread.interact`; the others get it at their next turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnnouncementCreateBody {
    /// Its title.
    pub title: String,
    /// Its text.
    pub body: String,
    /// What kind of news it is: a notice when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<AnnouncementCategory>,
    /// How much it matters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<AnnouncementSeverity>,
    /// Whom to send it to: all when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub targets: Option<AnnouncementSendAudience>,
    /// How it reaches a thread that is at work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<AnnouncementDelivery>,
    /// What it does where it may not reach into a running turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<AnnouncementFallback>,
    /// Whether it wakes a thread that is idle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_idle: Option<bool>,
}

/// The announcements sent to a place: one in every open topic that is not archived
/// and was not made private below it, each applying its own
/// resolved settings for what the request left out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaceAnnouncementCreateReply {
    /// What they have in common.
    pub group_id: Id,
    /// The announcements.
    pub announcements: Vec<Announcement>,
    /// How many topics were left out.
    pub topics_skipped: i64,
}

crate::endpoints! {
    /// Brings a client up to date on a topic: the topic, its threads, the jobs about them and its announcements, or what changed since a sequence.
    TopicSyncGet: GET "/topics/{id}/sync", Permission("node.view"), query TopicSyncGetQuery, none, json TopicSync, 200;
    /// Lists the environments that a topic's threads run in and those whose home it is, the default first, each as far as the caller may see it.
    TopicEnvironmentList: GET "/topics/{id}/environments", Permission("node.view"), none, none, json Vec<Environment>, 200;
    /// Names the environment that new threads of a topic join, or takes it away.
    TopicDefaultEnvironmentSet: PUT "/topics/{id}/default-environment", Permission("topic.manage"), none, json TopicDefaultEnvironmentSetBody, json Topic, 200;
    /// Lists where a thread of a topic could run, for the caller: the hosts they may use for a new environment, the most free memory first, then the existing environments they manage.
    TopicPlaceList: GET "/topics/{id}/places", Permission("topic.interact"), query PlaceListQuery, none, json Vec<Place>, 200;
    /// Lists announcements in the readable topics of an org, project or topic, newest first.
    NodeAnnouncementList: GET "/nodes/{id}/announcements", Permission("node.view"), query NodeAnnouncementListQuery, none, json Page<Announcement>, 200;
    /// Announces to the open topics of an org, project or topic; private branches are passed over and each topic applies its settings.
    NodeAnnouncementCreate: POST "/nodes/{id}/announcements", Rule("node.interact; steering and interrupting reach only threads where the sender holds thread.interact"), none, json AnnouncementCreateBody, json PlaceAnnouncementCreateReply, 201;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_announcement_went_to_an_audience_or_to_one_thread() {
        let audience: AnnouncementTargets = serde_json::from_value(json!("none")).unwrap();
        assert_eq!(
            audience,
            AnnouncementTargets::Audience(AnnouncementAudience::Nobody)
        );
        let thread: AnnouncementTargets =
            serde_json::from_value(json!({ "thread_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b" }))
                .unwrap();
        assert!(matches!(thread, AnnouncementTargets::Thread(_)));
        assert!(serde_json::from_value::<AnnouncementTargets>(json!("some")).is_err());
    }

    #[test]
    fn an_announcement_from_a_thread_names_the_thread_and_counts_what_it_did() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b",
            "topic_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c",
            "group_id": null, "category": "milestone", "title": "Schema is frozen",
            "body": "Migrations are in.", "severity": "important",
            "from": { "thread": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6d", "name": "Schema" } },
            "targets": "active", "files": [{ "name": "schema.sql", "size": 2048 }],
            "delivery": "steer", "fallback": "next_turn", "wake_idle": false,
            "delivered": { "interrupted": 0, "steered": 2, "queued": 1, "deferred": 0 },
            "created_at": "2026-10-08T17:04:05.123Z", "topic_seq": 17
        });
        let announcement: Announcement = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(announcement.delivery, AnnouncementDelivery::Steer);
        assert!(matches!(announcement.from, AnnouncementFrom::Thread(_)));
        assert_eq!(announcement.delivered.steered, 2);
        assert_eq!(serde_json::to_value(&announcement).unwrap(), written);
    }

    #[test]
    fn a_sender_may_send_to_all_or_to_the_active_threads_and_to_nobody_else() {
        let body: AnnouncementCreateBody = serde_json::from_value(
            json!({ "title": "Heads up", "body": "Rebase first", "targets": "active", "wake_idle": true }),
        )
        .unwrap();
        assert_eq!(body.targets, Some(AnnouncementSendAudience::Active));
        assert!(
            serde_json::from_value::<AnnouncementCreateBody>(
                json!({ "title": "x", "body": "y", "targets": "none" })
            )
            .is_err(),
            "followers alone hear of a milestone, not of what a person sends"
        );
    }

    #[test]
    fn the_default_environment_of_a_topic_can_be_taken_away_with_null() {
        let clear: TopicDefaultEnvironmentSetBody =
            serde_json::from_value(json!({ "environment_id": null })).unwrap();
        assert_eq!(clear.environment_id, None);
        let set: TopicDefaultEnvironmentSetBody = serde_json::from_value(
            json!({ "environment_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b" }),
        )
        .unwrap();
        assert!(set.environment_id.is_some());
        assert!(
            serde_json::from_value::<TopicDefaultEnvironmentSetBody>(
                json!({ "environment_id": null, "thread_id": null })
            )
            .is_err()
        );
    }
}
