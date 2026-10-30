//! Favorites, recents, read state and drafts; and what the interface keeps for a person: named arrangements, hints closed and tutorials finished, and the viewer chosen for a kind of file.
//!
//! Everything here acts on the caller and on nobody else. A change is also
//! delivered on the person's own realtime feed, so that what one device does
//! shows on the others; what a device arranges for itself, such as the sizes
//! of its panes, stays on that device.

use std::collections::BTreeMap;

use orch_types::{Id, Json, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::SubscriptionLevel;
use crate::nodes::{NodeRef, TopicSort, TopicStatusFilter};
use crate::routes::Access::{Permission, Rule, User};
use crate::threads::{ContentPart, Delivery};

/// The most bytes of the value of one key of a person's interface state.
pub const PERSONAL_STATE_MAX_BYTES: i64 = 64 * 1024;

/// A message that was begun in a thread's composer and not sent. Drafts are
/// private to their author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Draft {
    /// Private selected native action and arguments, if any.
    #[serde(default)]
    pub native_action: Option<crate::input_schedules::NativeInvocation>,
    /// What was written.
    pub content: Vec<ContentPart>,
    /// How it is to be delivered.
    pub delivery: Delivery,
    /// Frozen reviewed hunks and line anchors, private to this author.
    pub review_comments: Vec<crate::source::ReviewComment>,
    /// When it was last saved.
    pub updated_at: Time,
}

/// A node a person keeps close, with the way down to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Favorite {
    /// The node.
    pub node: NodeRef,
    /// The way down to it.
    pub path: Vec<NodeRef>,
    /// Its place among the favorites; the list is in order.
    pub position: String,
    /// For a project: the view it was starred with, opened as that view's ordinary address; `null`
    /// stars the place alone.
    #[serde(default)]
    pub view: Option<ProjectView>,
}

/// How a project's topics were last looked at: a list or a board.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectViewKind {
    /// The list.
    List,
    /// The board.
    Board,
}

/// What a project's topics are narrowed to by what needs somebody.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProjectViewAttention {
    /// Waiting for an approval or unable to go on.
    NeedsAttention,
    /// A turn running.
    Running,
    /// Waiting for an approval.
    WaitingApproval,
    /// Unable to go on.
    Blocked,
}

/// The last List or Board a person had of a project, with its filters: what the project opens with for
/// them on every device, unless an address says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectView {
    /// A list or a board.
    pub view: ProjectViewKind,
    /// Open, done or all topics.
    pub status: TopicStatusFilter,
    /// The tags every topic shown carries.
    pub tag_ids: Vec<Id>,
    /// What needs somebody, where it is narrowed to that.
    pub attention: Option<ProjectViewAttention>,
    /// The order.
    pub sort: TopicSort,
    /// What a name or a description holds.
    pub q: String,
    /// Whether archived topics are shown.
    pub show_archived: bool,
    /// Whether the topics of the folders below are shown with those of the project.
    pub with_folders: bool,
}

/// The keyboard actions of the application, by the name the browser binds each under, with the keys it
/// ships with: `Mod` is Command on Apple's systems and Control elsewhere, and `G I` is two keys in a row.
/// A person's own keys (`keys` of their interface state) are checked against these. The right bar is not
/// `Mod+.`: on Windows Ctrl+. switches the punctuation of Microsoft's Chinese input method.
pub const KEY_ACTIONS: &[(&str, &str)] = &[
    ("finder", "Mod+K"),
    ("search", "Mod+/"),
    ("find", "Mod+F"),
    ("left-bar", "Mod+\\"),
    ("right-bar", "Mod+Shift+."),
    ("bottom", "Mod+J"),
    ("next-area", "F6"),
    ("previous-area", "Shift+F6"),
    ("tab-1", "Alt+1"),
    ("tab-2", "Alt+2"),
    ("tab-3", "Alt+3"),
    ("tab-4", "Alt+4"),
    ("tab-5", "Alt+5"),
    ("tab-6", "Alt+6"),
    ("tab-7", "Alt+7"),
    ("tab-8", "Alt+8"),
    ("tab-9", "Alt+9"),
    ("close-tab", "Alt+W"),
    ("focus-mode", ""),
    ("help", "F1"),
    ("inbox", "G I"),
    ("approvals", "G A"),
    ("messages", "G M"),
    ("list", "?"),
    ("tree-up", "Alt+Shift+ArrowLeft"),
    ("tree-only", "Alt+Shift+ArrowRight"),
    ("tree-filter", "Ctrl+Alt+F"),
    ("previous-thread", "Alt+ArrowUp"),
    ("next-thread", "Alt+ArrowDown"),
    ("previous-unread", "Alt+Shift+ArrowUp"),
    ("next-unread", "Alt+Shift+ArrowDown"),
    ("document-save", "Mod+S"),
];

const fn single_keys_as_shipped() -> bool {
    true
}

/// A person's keys (`keys` of their interface state): whether keys without a modifier act, and the keys
/// they gave actions of their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ShortcutState {
    /// Whether keys without Ctrl, Alt or Command (a letter, a symbol, a sequence of them, End) act; true
    /// as shipped.
    #[serde(default = "single_keys_as_shipped")]
    pub single_key: bool,
    /// Keys by action: an action left out has the keys it ships with, and `null` leaves it without keys.
    #[serde(default)]
    pub bindings: BTreeMap<String, Option<String>>,
}

/// One key pressed with its modifiers, as a shortcut names it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Chord {
    key: String,
    command: bool,
    ctrl: bool,
    alt: bool,
    shift: bool,
}

/// The keys a shortcut may name besides a character.
const NAMED_KEYS: &[&str] = &[
    "f1",
    "f2",
    "f3",
    "f4",
    "f5",
    "f6",
    "f7",
    "f8",
    "f9",
    "f10",
    "f11",
    "f12",
    "arrowup",
    "arrowdown",
    "arrowleft",
    "arrowright",
    "home",
    "end",
    "pageup",
    "pagedown",
    "enter",
    "escape",
    "tab",
    "space",
    "backspace",
    "delete",
    "insert",
];

/// The keys a control moves with by itself: never a shortcut without Ctrl, Alt or Command.
const MOVING_KEYS: &[&str] = &[
    "tab",
    "enter",
    "escape",
    "space",
    "backspace",
    "delete",
    "arrowup",
    "arrowdown",
    "arrowleft",
    "arrowright",
    "home",
    "end",
    "pageup",
    "pagedown",
];

/// Reads keys as people write them (`Mod+Shift+.`, `G I`, `Mod++`), or says why they are no keys.
fn chords(text: &str) -> Result<Vec<Chord>, &'static str> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.is_empty() || parts.len() > 2 {
        return Err("are one key, or two in a row");
    }
    parts
        .into_iter()
        .map(|part| {
            let mut chord = Chord {
                key: String::new(),
                command: false,
                ctrl: false,
                alt: false,
                shift: false,
            };
            // A literal plus is written last: `Mod++`, or alone.
            let named = if part == "+" {
                chord.key = "+".to_owned();
                ""
            } else if let Some(rest) = part.strip_suffix("++") {
                chord.key = "+".to_owned();
                rest
            } else {
                part
            };
            for token in named.split('+').filter(|token| !token.is_empty()) {
                match token.to_lowercase().as_str() {
                    "mod" => chord.command = true,
                    "ctrl" => chord.ctrl = true,
                    "alt" => chord.alt = true,
                    "shift" => chord.shift = true,
                    name if chord.key.is_empty() => chord.key = name.to_owned(),
                    _ => return Err("name one key in each step"),
                }
            }
            let character = chord.key.chars().count() == 1;
            if !character && !NAMED_KEYS.contains(&chord.key.as_str()) {
                return Err("name no key there is");
            }
            let plain = !chord.command && !chord.ctrl && !chord.alt;
            if plain && MOVING_KEYS.contains(&chord.key.as_str()) {
                return Err("are a key a control moves with by itself");
            }
            Ok(chord)
        })
        .collect()
}

/// Whether `start` is where `whole` begins, or the same keys.
fn begins(whole: &[Chord], start: &[Chord]) -> bool {
    start.len() <= whole.len() && whole.iter().zip(start).all(|(one, other)| one == other)
}

impl ShortcutState {
    /// Checks the keys: every action named is one there is, its keys are keys, and no two actions have the
    /// same keys or keys that begin the other's. Gives the member that is wrong, and why, naming the other
    /// action where two meet.
    ///
    /// # Errors
    ///
    /// The member (`bindings.<action>`) and the reason, when the keys cannot be kept.
    pub fn check(&self) -> Result<(), (String, String)> {
        if let Some(unknown) = self
            .bindings
            .keys()
            .find(|name| !KEY_ACTIONS.iter().any(|(action, _)| action == name))
        {
            return Err((format!("bindings.{unknown}"), "is no action".to_owned()));
        }
        let mut held: Vec<(&str, Vec<Chord>)> = Vec::new();
        for &(action, shipped) in KEY_ACTIONS {
            let keys = match self.bindings.get(action) {
                Some(Some(own)) => own.as_str(),
                Some(None) => continue,
                None if shipped.is_empty() => continue,
                None => shipped,
            };
            let read =
                chords(keys).map_err(|why| (format!("bindings.{action}"), why.to_owned()))?;
            held.push((action, read));
        }
        for (at, (action, mine)) in held.iter().enumerate() {
            for (other, theirs) in held.iter().skip(at + 1) {
                if begins(mine, theirs) || begins(theirs, mine) {
                    // The action the person chose keys for is the one that is wrong; where both were chosen, the first.
                    let (chosen, kept) = if self.bindings.contains_key(*action) {
                        (action, other)
                    } else {
                        (other, action)
                    };
                    return Err((
                        format!("bindings.{chosen}"),
                        format!(
                            "has keys that {kept} has, or that begin or are the beginning of its keys"
                        ),
                    ));
                }
            }
        }
        Ok(())
    }
}

/// How far a person has read a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReadState {
    /// The thread.
    pub thread_id: Id,
    /// The selected history whose read position this row describes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    /// The last sequence they read.
    pub last_read_seq: Seq,
}

/// How a subscription came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionSource {
    /// Made by the system, for something the person took part in.
    Auto,
    /// Chosen by the person.
    Manual,
}

/// One subscription row of a person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeSubscription {
    /// The node.
    pub node_id: Id,
    /// How much they want to be told.
    pub level: SubscriptionLevel,
    /// How the row came to be.
    pub source: SubscriptionSource,
}

pub use crate::chats::{ChatLevel, ChatState};

/// The keys under which a person's interface remembers things for them across devices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PersonalStateKey {
    /// Tutorials finished and hints closed.
    Help,
    /// Arrangements of the workbench saved by name.
    Layouts,
    /// What a kind of file opens with.
    Viewers,
    /// How a conversation is shown to them: how much of the agent's work opens on its own, and
    /// whether a difference is drawn in one column or in two.
    Timeline,
    /// Which sidebar sections are folded and whether its tree shows documents, the place last in,
    /// and the order of each org's tree.
    Sidebar,
    /// The last List or Board of each project, by its id.
    ProjectViews,
    /// Whether single keys act, and the person's own bindings.
    Keys,
    /// The last few emoji chosen, newest first.
    Reactions,
    /// How the Inbox groups its rows.
    Inbox,
}

/// Where to put a favorite.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeFavoriteSetBody {
    /// The favorite it goes after; first in the list when `null`, and last
    /// when left out.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<Id>")]
    pub after_node_id: Option<Option<Id>>,
    /// For a project: the view it is starred with; `null` stars the place alone, and leaving it out
    /// keeps the view it has.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<ProjectView>")]
    pub view: Option<Option<ProjectView>>,
}

/// Notes that a person went to a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeRecentCreateBody {
    /// The node.
    pub node_id: Id,
}

/// One personally saved or visited thing; identifiers belonging to different kinds stay distinct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PersonalTarget {
    /// Its kind, including people, conversations, documents and resources.
    pub kind: crate::search::NameKind,
    /// Its stable identifier.
    pub id: Id,
}

/// Where a favorite of any kind goes in the single personal list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeItemFavoriteSetBody {
    /// The favorite it follows; null puts it first, absence puts it last.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<PersonalTarget>")]
    pub after: Option<Option<PersonalTarget>>,
}

/// The threads whose read position is wanted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeReadStateListQuery {
    /// Only the threads of this org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// The subscriptions wanted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeSubscriptionListQuery {
    /// Only the nodes of this org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// Says how far the caller has read a thread. The stored value is the larger
/// of the old and the new.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThreadReadBody {
    /// Read-only scope whose cursor is advanced; omission follows active at admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_id: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_revision: Option<i64>,
    /// The sequence read.
    pub seq: Seq,
}

/// What the stored read position of a thread is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ThreadReadReply {
    /// The current execution's reader-specific icon after this read was stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_display: Option<crate::threads::ThreadDisplay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<crate::threads::ThreadContinuationProjection>,
    /// The stored position.
    pub last_read_seq: Seq,
}

/// Everything a person's interface remembers, by key.
pub type PersonalState = BTreeMap<String, Json>;

crate::endpoints! {
    /// Lists the caller's favorites, in order.
    MeFavoriteList: GET "/me/favorites", User, none, none, json Vec<Favorite>, 200;
    /// Makes a node a favorite, or moves it within the list.
    MeFavoriteSet: PUT "/me/favorites/{node_id}", Permission("node.view"), none, json MeFavoriteSetBody, none, 204;
    /// Takes a node out of the favorites.
    MeFavoriteDelete: DELETE "/me/favorites/{node_id}", User, none, none, none, 204;
    /// Notes that the caller went to a node; the client calls it on navigation.
    MeRecentCreate: POST "/me/recents", User, none, json MeRecentCreateBody, none, 204;
    /// Stars or reorders a readable thing of any kind in the caller's own favorites.
    MeItemFavoriteSet: PUT "/me/favorites/{kind}/{id}", User, none, json MeItemFavoriteSetBody, none, 204;
    /// Removes only this kind/id favorite, including when the target is no longer readable.
    MeItemFavoriteDelete: DELETE "/me/favorites/{kind}/{id}", User, none, none, none, 204;
    /// Records one visit to a readable thing, using the server's clock and decaying count.
    MeVisitCreate: POST "/me/visits", User, none, json PersonalTarget, none, 204;
    /// Lists how far the caller has read their threads.
    MeReadStateList: GET "/me/read-states", User, query MeReadStateListQuery, none, json Vec<ReadState>, 200;
    /// Lists the caller's subscriptions.
    MeSubscriptionList: GET "/me/subscriptions", User, query MeSubscriptionListQuery, none, json Vec<MeSubscription>, 200;
    /// Lists the caller's state in every chat where they have one.
    MeChatStateList: GET "/me/chat-states", User, none, none, json Vec<ChatState>, 200;
    /// Reads what the caller's interface remembers for them across devices.
    MeStateGet: GET "/me/state", User, none, none, json PersonalState, 200;
    /// Replaces the whole value of one key of that state.
    MeStateSet: PUT "/me/state/{key}", User, none, json Json, none, 204;
    /// Keeps the caller's last view of one project, beside those of the others.
    MeProjectViewSet: PUT "/me/state/project_views/{project_id}", Rule("the caller, who may view the project"), none, json ProjectView, none, 204;
    /// Forgets the caller's last view of one project.
    MeProjectViewDelete: DELETE "/me/state/project_views/{project_id}", User, none, none, none, 204;
    /// Records how far the caller has read a thread.
    ThreadRead: POST "/threads/{id}/read", Permission("thread.view"), none, json ThreadReadBody, json ThreadReadReply, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keys(value: serde_json::Value) -> ShortcutState {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn the_shipped_keys_meet_nowhere_and_a_persons_keys_are_checked_against_them() {
        assert_eq!(keys(json!({})).check(), Ok(()));
        let own = keys(json!({ "bindings": { "inbox": null, "finder": "mod+p" } }));
        assert!(own.single_key);
        assert_eq!(own.check(), Ok(()));
        // Keys another action has, or that begin its keys, are refused at the action they were chosen for, naming the other.
        let (member, why) = keys(json!({ "single_key": false, "bindings": { "search": "Mod+K" } }))
            .check()
            .unwrap_err();
        assert_eq!(member, "bindings.search");
        assert!(why.contains("finder"), "{why}");
        let begun = keys(json!({ "bindings": { "search": "G" } }))
            .check()
            .unwrap_err();
        assert_eq!(begun.0, "bindings.search");
        assert!(begun.1.contains("inbox"), "{}", begun.1);
        // Keys an action no longer has are free.
        assert_eq!(
            keys(json!({ "bindings": { "inbox": null, "search": "G I" } })).check(),
            Ok(())
        );
        for wrong in ["ArrowUp", "G O T", "Mod+Nothing", "Mod+K+J", ""] {
            let fault = keys(json!({ "bindings": { "finder": wrong } }))
                .check()
                .unwrap_err();
            assert_eq!(fault.0, "bindings.finder", "{wrong}");
        }
        assert_eq!(
            keys(json!({ "bindings": { "launch": "Mod+L" } })).check(),
            Err(("bindings.launch".to_owned(), "is no action".to_owned()))
        );
    }

    #[test]
    fn focus_mode_has_no_default_but_its_chosen_binding_keeps_the_ordinary_validation() {
        assert_eq!(
            KEY_ACTIONS
                .iter()
                .filter(|(name, _)| *name == "focus-mode")
                .count(),
            1
        );
        assert!(KEY_ACTIONS.contains(&("focus-mode", "")));
        assert_eq!(keys(json!({})).check(), Ok(()));
        assert_eq!(
            keys(json!({"bindings":{"focus-mode":null}})).check(),
            Ok(())
        );
        assert_eq!(
            keys(json!({"bindings":{"focus-mode":"Ctrl+Alt+Shift+F10"}})).check(),
            Ok(())
        );
        for chosen in ["", " ", "Alt", "Escape", "Alt+W", "G"] {
            let Err((member, _)) = keys(json!({"bindings":{"focus-mode":chosen}})).check() else {
                panic!("an invalid or conflicting chosen key cannot become an unbound default");
            };
            assert_eq!(member, "bindings.focus-mode");
        }
    }

    #[test]
    fn a_favorite_goes_first_when_null_and_last_when_left_out() {
        let first: MeFavoriteSetBody =
            serde_json::from_value(json!({ "after_node_id": null })).unwrap();
        let last: MeFavoriteSetBody = serde_json::from_value(json!({})).unwrap();
        let after: MeFavoriteSetBody = serde_json::from_value(
            json!({ "after_node_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b" }),
        )
        .unwrap();
        assert_eq!(first.after_node_id, Some(None));
        assert_eq!(last.after_node_id, None);
        assert!(matches!(after.after_node_id, Some(Some(_))));
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            json!({ "after_node_id": null })
        );
        assert_eq!(serde_json::to_value(&last).unwrap(), json!({}));
    }

    #[test]
    fn a_draft_keeps_the_way_it_was_to_be_delivered() {
        let draft: Draft = serde_json::from_value(json!({
            "content": [{ "type": "text", "text": "Please also run the tests" }],
            "delivery": "steer",
            "review_comments": [],
            "updated_at": "2026-10-08T17:04:05.123Z"
        }))
        .unwrap();
        assert_eq!(draft.delivery, Delivery::Steer);
        assert_eq!(draft.content.len(), 1);
    }

    #[test]
    fn the_interface_state_is_kept_under_nine_keys() {
        assert_eq!(
            serde_json::to_value([
                PersonalStateKey::Help,
                PersonalStateKey::Layouts,
                PersonalStateKey::Viewers,
                PersonalStateKey::Timeline,
                PersonalStateKey::Sidebar,
                PersonalStateKey::ProjectViews,
                PersonalStateKey::Keys,
                PersonalStateKey::Reactions,
                PersonalStateKey::Inbox
            ])
            .unwrap(),
            json!([
                "help",
                "layouts",
                "viewers",
                "timeline",
                "sidebar",
                "project_views",
                "keys",
                "reactions",
                "inbox"
            ])
        );
    }

    #[test]
    fn a_project_view_reads_as_the_screen_writes_it_and_a_star_keeps_or_clears_it() {
        let view: ProjectView = serde_json::from_value(json!({
            "view": "board", "status": "all", "tag_ids": [], "attention": "needs_attention",
            "sort": "name", "q": "login", "show_archived": false, "with_folders": true
        }))
        .unwrap();
        assert_eq!(view.attention, Some(ProjectViewAttention::NeedsAttention));
        let kept: MeFavoriteSetBody = serde_json::from_value(json!({})).unwrap();
        let alone: MeFavoriteSetBody = serde_json::from_value(json!({ "view": null })).unwrap();
        assert_eq!(kept.view, None);
        assert_eq!(alone.view, Some(None));
    }
}
