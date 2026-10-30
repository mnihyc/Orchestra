//! Users with their state; invitations as a link to share or a personal link for one address, within what a root org allows of each; and what an invitation shows before it is accepted.
//!
//! Other people see a person as a [`UserRef`]; only the person themself reads
//! their own [`Me`], which adds what they signed in with and how they like the
//! interface. An [`Invitation`] is either a shared link, good for a number of
//! people, or a personal link bound to one address; what it would give is
//! shown to anybody who holds the token as an [`InvitationPreview`] before it
//! is accepted.

use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::Role;
use crate::common::{Page, PageQuery};
use crate::nodes::{NodeKind, NodeRef, Org};
use crate::routes::Access::{Public, Rule, User};

/// The number of colours in the palette that avatars and tags draw from. A colour is stored as its
/// index, from 0 up to one less than this, so that a page can follow the light or the dark theme.
pub const COLOR_COUNT: u32 = 12;
/// The fewest characters of a search for people.
pub const USER_SEARCH_MIN_CHARS: u32 = 2;
/// The most people a search for people returns, whatever is asked.
pub const USER_SEARCH_MAX_RESULTS: u32 = 20;

/// The picture a person has in place of a photograph: a few letters on a
/// colour.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Avatar {
    /// The letters shown.
    pub text: String,
    /// The colour behind them, an index below [`COLOR_COUNT`].
    pub color: u32,
}

/// Whether a user is somebody who signs in or an identity made for automation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UserKind {
    /// A person.
    Human,
    /// A service identity that works through tokens.
    Service,
}

/// A person as others see them, wherever one is named.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct UserRef {
    /// The user's identifier.
    pub id: Id,
    /// A person or a service identity.
    pub kind: UserKind,
    /// The name they sign in with and are mentioned by.
    pub username: String,
    /// Their address in the visible context; otherwise their public username as `@name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// The name shown beside what they did.
    pub display_name: String,
    /// Their picture.
    pub avatar: Avatar,
}

/// What a mailed code may be for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EmailCodeUse {
    /// The first step of a sign-in.
    SignIn,
    /// The second step of a sign-in, after another proof.
    SecondStep,
}

/// The languages the interface is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum Locale {
    /// English.
    #[serde(rename = "en")]
    En,
    /// Simplified Chinese.
    #[serde(rename = "zh-CN")]
    ZhCn,
}

/// The colour scheme a person chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Whatever their device is set to.
    System,
    /// Light.
    Light,
    /// Dark.
    Dark,
}

/// How closely the interface is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    /// The roomier setting.
    Comfortable,
    /// The tighter setting.
    Compact,
}

/// An identity at an identity provider that a person linked to their user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeIdentity {
    /// The short key of the identity provider.
    pub idp_key: String,
    /// The name of the identity provider as shown.
    pub idp_name: String,
    /// The address the provider gave for them, if it gave one.
    pub email: Option<String>,
    /// When they linked it.
    pub linked_at: Time,
}

/// Where a person's time zone comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimezoneMode {
    /// The browser they use reports it, and a device in another zone changes it.
    Device,
    /// They chose it, and it stays the same on every device; a device's report never changes it.
    #[default]
    Fixed,
}

/// The signed-in person, as they alone read themself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Me {
    /// Who they are to others.
    #[serde(flatten)]
    pub user: UserRef,
    /// Their address, once they gave one.
    pub email: Option<String>,
    /// Whether that address was confirmed.
    pub email_verified: bool,
    /// Whether they have a password to sign in with.
    pub has_password: bool,
    /// Whether an authenticator app is their second step.
    pub totp_enabled: bool,
    /// A new address the person asked for, which stays pending until it is confirmed.
    pub email_pending: Option<String>,
    /// The uses a mailed code may have for them: only those that every org of theirs allows, and
    /// none while they have no confirmed address.
    pub email_code: Vec<EmailCodeUse>,
    /// True once they chose a mailed code as a second step.
    pub email_code_second_step: bool,
    /// Whether an org of theirs, or the system where they belong to no team org, requires a second
    /// step of them: the team orgs that do wait behind its setup, and behind giving it in each session.
    pub second_step_required: bool,
    /// Where that requirement is set: the team orgs, or the system.
    pub second_step_sources: Vec<crate::nodes::NodeOrSystem>,
    /// Whether they are an administrator of the whole system.
    pub is_system_admin: bool,
    /// The language they chose.
    pub locale: Locale,
    /// Their time zone, as an IANA name: the last valid one, also used while they are away.
    pub timezone: String,
    /// Whether that zone follows the device they use or is one they chose.
    pub timezone_mode: TimezoneMode,
    /// Their colour scheme.
    pub theme: Theme,
    /// How closely they set the interface.
    pub density: Density,
    /// Their personal organization, absent when it was not created.
    pub personal_org_id: Option<Id>,
    /// When the user was made.
    pub created_at: Time,
    /// The identities they linked.
    pub identities: Vec<MeIdentity>,
}

/// What a link of invitation is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvitationKind {
    /// To the system itself.
    System,
    /// To an org, at whatever level it lies, as a member of its tree.
    Org,
    /// To one project, topic or thread, without membership of its root org.
    Node,
    /// To a group chat, as a guest.
    Chat,
}

/// The two forms an invitation takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvitationForm {
    /// A shared link: whoever holds it may use it, up to a number of people.
    Link,
    /// A personal link for one address, good for one use.
    Email,
}

pub use crate::chats::{ChatKind, ChatRef};

/// An invitation as the people who manage it read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Invitation {
    /// The invitation's identifier.
    pub id: Id,
    /// What it invites to.
    pub kind: InvitationKind,
    /// The org it invites into, for an org invitation.
    pub org: Option<NodeRef>,
    /// The project, topic or thread it invites to, for a node invitation.
    pub node: Option<NodeRef>,
    /// The chat it invites to, for a chat invitation.
    pub chat: Option<ChatRef>,
    /// The role it gives, where it gives one.
    pub role: Option<Role>,
    /// The permissions it names on a node's page, where it names any.
    pub permissions: Vec<String>,
    /// A shared link, or a personal link.
    pub form: InvitationForm,
    /// The one address a personal link is for.
    pub email: Option<String>,
    /// How many people may use it.
    pub max_uses: i64,
    /// How many have used it.
    pub uses: i64,
    /// A note its maker left.
    pub note: String,
    /// When it stops being good.
    pub expires_at: Time,
    /// Whether it was taken back.
    pub revoked: bool,
    /// Who made it.
    pub created_by: UserRef,
    /// When it was made.
    pub created_at: Time,
}

/// The org an invitation brings a person into, as its preview names it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvitationPreviewOrg {
    /// The org's name.
    pub name: String,
    /// The org's short name.
    pub slug: String,
}

/// What a node or chat invitation points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InvitedKind {
    /// An org.
    Org,
    /// A project.
    Project,
    /// A topic.
    Topic,
    /// A thread.
    Thread,
    /// A chat.
    Chat,
}

impl From<NodeKind> for InvitedKind {
    fn from(kind: NodeKind) -> Self {
        match kind {
            NodeKind::Org => Self::Org,
            NodeKind::Project => Self::Project,
            NodeKind::Topic => Self::Topic,
            NodeKind::Thread => Self::Thread,
        }
    }
}

/// The thing a node or chat invitation points at, by kind and name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvitationPreviewWhat {
    /// What it is.
    pub kind: InvitedKind,
    /// Its name.
    pub name: String,
}

/// What an invitation shows to whoever holds its token, before it is accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvitationPreview {
    /// What it invites to.
    pub kind: InvitationKind,
    /// The org, for an org invitation.
    pub org: Option<InvitationPreviewOrg>,
    /// The node or chat, for those invitations.
    pub what: Option<InvitationPreviewWhat>,
    /// The role accepting it gives.
    pub role: Option<Role>,
    /// The permissions accepting it gives.
    pub permissions: Vec<String>,
    /// Who invited.
    pub inviter: UserRef,
    /// Whether it is bound to one address.
    pub email_bound: bool,
    /// When it stops being good.
    pub expires_at: Time,
}

/// Searches for people by the start of a username or a part of a display name
/// among those who share an org with the caller, and by an exact username or
/// address anywhere. It never lists everybody.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserSearchQuery {
    /// What to look for, at least [`USER_SEARCH_MIN_CHARS`] characters.
    pub q: String,
    /// Look among the members of this org, at whatever level it lies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// Makes an invitation. With `email` it is a personal link; without, a shared
/// link. What may be asked for is bounded by the policies of the root org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvitationCreateBody {
    /// What to invite to.
    pub kind: InvitationKind,
    /// The org, for an org invitation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
    /// The project, topic or thread, for a node invitation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
    /// The group chat, for a chat invitation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<Id>,
    /// The role to give; a viewer when left out of an org invitation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// The permissions to name on a node's page; its chat alone when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<String>>,
    /// The one address a personal link is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// How many people a shared link may serve; one when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<i64>,
    /// How long it stays good; the policy's default when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_hours: Option<i64>,
    /// A note for whoever manages invitations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A made invitation, with the one chance to read its token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvitationCreateReply {
    /// The invitation.
    pub invitation: Invitation,
    /// The secret of the link, shown once.
    pub token: String,
    /// The address of the link to give out.
    pub url: String,
    /// True when a personal link went out by mail. It is false where the org cannot send mail, and
    /// then the caller hands the link on.
    pub sent: bool,
}

/// The invitations of a place.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvitationListQuery {
    /// Only the invitations into this org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// Accepts an invitation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InvitationAcceptBody {
    /// The secret of the link.
    pub token: String,
}

/// What accepting an invitation brought the person into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InvitationAcceptReply {
    /// The org they joined, for an org invitation.
    pub org: Option<Org>,
    /// The node they may open after accepting a node invitation, where its View is held.
    pub node: Option<NodeRef>,
    /// The chat they may open after a group or chat-only node invitation.
    pub chat: Option<ChatRef>,
}

/// Who may open a person's page, subject to recorded member-management inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProfileAudience {
    /// Anybody signed in.
    #[default]
    System,
    /// People who share a tree of orgs with the person.
    Orgs,
    /// The person alone.
    Private,
}

/// A link the person chose to put on their page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfileLink {
    /// The person's label, one to forty characters.
    pub label: String,
    /// An HTTPS address, at most five hundred characters.
    pub url: String,
}

/// The switches of the page, returned only to its person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileSettings {
    /// Whether others see their local time.
    pub show_time: bool,
    /// Whether others see their visible work.
    pub show_activity: bool,
    /// Who opens the page.
    pub audience: ProfileAudience,
}

/// The person's current local time, without exposing a hidden time zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileLocalTime {
    /// Their IANA time zone.
    pub timezone: String,
    /// The current offset from UTC in minutes, including daylight saving.
    pub utc_offset_minutes: i64,
}

/// One visible pin; exactly one of its members is present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProfilePin {
    /// A place or a thread.
    pub node: Option<crate::nodes::Node>,
    /// A document.
    pub document: Option<crate::documents::DocumentRef>,
}

/// A person's page as one reader may see it; it grants no access.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UserProfile {
    /// Who the page belongs to.
    pub user: UserRef,
    /// When they joined.
    pub joined_at: Time,
    /// Whether the reader is that person.
    pub is_me: bool,
    /// Their own words, in the chat Markdown subset.
    pub bio: String,
    /// Their pronouns, or an empty string.
    pub pronouns: String,
    /// Their chosen links.
    pub links: Vec<ProfileLink>,
    /// Their local time, if they show it.
    pub local_time: Option<ProfileLocalTime>,
    /// Their domains in shared trees, only where the reader may view them.
    pub member_of: Vec<NodeRef>,
    /// Their personal org, only where the reader may view it.
    pub personal_org: Option<NodeRef>,
    /// Their visible pins in the order they chose, with no count of omitted pins.
    pub pins: Vec<ProfilePin>,
    /// Whether this reader may ask for work by day.
    pub activity: bool,
    /// Whether a direct chat with them is possible.
    pub can_message: bool,
    /// The org whose member manager opened an otherwise closed page.
    pub inspected: Option<NodeRef>,
    /// The person's switches, for them alone.
    pub settings: Option<ProfileSettings>,
}

/// One UTC day with visible work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileActivityDay {
    /// The day as YYYY-MM-DD.
    pub day: String,
    /// Turns attributed to this person in threads the reader may view.
    pub turns: i64,
}

/// A visible topic the person recently worked in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileActivityRecent {
    /// The topic, only where the reader may view it.
    pub node: NodeRef,
    /// The last visible work in it.
    pub last_at: Time,
}

/// The last fifty-three weeks of work, cut to the reader's own access before summing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProfileActivity {
    /// First UTC day, inclusive.
    pub from: String,
    /// Last UTC day, inclusive.
    pub to: String,
    /// Days with visible work only.
    pub days: Vec<ProfileActivityDay>,
    /// The sum of the visible days.
    pub total: i64,
    /// At most eight visible topics, most recent first.
    pub recent: Vec<ProfileActivityRecent>,
}

/// Changes the person's page, keeping members that were not supplied.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeProfileUpdateBody {
    /// Their own words, at most two thousand characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bio: Option<String>,
    /// Pronouns, at most forty characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
    /// Replaces their links, at most six.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<Vec<ProfileLink>>,
    /// Whether to show their local time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_time: Option<bool>,
    /// Whether to show visible work to others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_activity: Option<bool>,
    /// Who opens the page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<ProfileAudience>,
}

/// The identifier of a pin, with an unambiguous kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ProfilePinInput {
    /// A place or thread the person may view now.
    Node(ProfilePinNode),
    /// A document the person may view now.
    Document(ProfilePinDocument),
}

/// A node selected as a pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfilePinNode {
    /// The node's identifier.
    pub node_id: Id,
}

/// A document selected as a pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProfilePinDocument {
    /// The document's identifier.
    pub document_id: Id,
}

/// Replaces every pin in the chosen order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeProfilePinsSetBody {
    /// At most six distinct things the person may view now.
    pub pins: Vec<ProfilePinInput>,
}

/// A current direct share between the caller and this person, readable by both now.
/// Group and inherited access are not enumerated, and omitted things have no count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UserShare {
    /// A place or thread directly given by either person to the other.
    Node {
        /// Its readable reference.
        node: NodeRef,
    },
    /// A document directly given by either person to the other.
    Document {
        /// Its readable reference.
        document: Box<crate::documents::DocumentRef>,
    },
    /// A document-only folder directly given by either person to the other.
    DocumentContainer {
        /// Its readable folder reference, opened through the folder metadata route.
        folder: crate::document_tree::DocumentContainerRef,
    },
}

crate::endpoints! {
    /// Opens a person's page with only the pins and places this reader may view.
    ProfileGet: GET "/profiles/{username}", User, none, none, json UserProfile, 200;
    /// Reads work by day in threads this reader may view.
    ProfileActivityGet: GET "/profiles/{username}/activity", User, none, none, json ProfileActivity, 200;
    /// Changes the signed-in person's page.
    MeProfileUpdate: PATCH "/me/profile", User, none, json MeProfileUpdateBody, json UserProfile, 200;
    /// Replaces the signed-in person's ordered pins.
    MeProfilePinsSet: PUT "/me/profile/pins", User, none, json MeProfilePinsSetBody, json UserProfile, 200;
    /// Finds people by the start of a name, or by an exact username or address.
    UserSearch: GET "/users/search", User, query UserSearchQuery, none, json Vec<UserRef>, 200;
    /// Reads one person as others see them.
    UserGet: GET "/users/{id}", User, none, none, json UserRef, 200;
    /// Lists current direct shares between these two people, cut to both people's View before paging, ordered by kind and identifier.
    UserSharesGet: GET "/users/{id}/shares", User, query PageQuery, none, json Page<UserShare>, 200;
    /// Creates an invitation, either a link to share or a personal link bound to one address.
    InvitationCreate: POST "/invitations", Rule("members.manage on the org, a system admin for the system, or what naming an outsider needs on a node or chat"), none, json InvitationCreateBody, json InvitationCreateReply, 200;
    /// Lists the invitations of a place, newest first.
    InvitationList: GET "/invitations", Rule("the same as making one"), query InvitationListQuery, none, json Vec<Invitation>, 200;
    /// Takes an invitation back.
    InvitationDelete: DELETE "/invitations/{id}", Rule("the same as making one"), none, none, none, 204;
    /// Shows what a link would give, before it is accepted.
    InvitationLookup: GET "/invitations/lookup/{token}", Public, none, none, json InvitationPreview, 200;
    /// Accepts an invitation as the signed-in person.
    InvitationAccept: POST "/invitations/accept", User, none, json InvitationAcceptBody, json InvitationAcceptReply, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_persons_share_card_pages_readable_references_without_an_omitted_count() {
        let id = "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02";
        let wire = json!({"items":[{"kind":"node","node":{"id":id,"kind":"topic","name":"Plan","redacted":false}}],"next_cursor":null});
        let page: Page<UserShare> = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(serde_json::to_value(page).unwrap(), wire);
    }

    #[test]
    fn a_pin_names_exactly_one_kind_and_a_page_patch_keeps_omitted_switches() {
        let id = "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02";
        assert!(
            serde_json::from_value::<ProfilePinInput>(json!({ "node_id": id, "document_id": id }))
                .is_err()
        );
        assert!(serde_json::from_value::<ProfilePinInput>(json!({})).is_err());
        let pin: ProfilePinInput = serde_json::from_value(json!({ "document_id": id })).unwrap();
        assert_eq!(
            serde_json::to_value(pin).unwrap(),
            json!({ "document_id": id })
        );
        let patch: MeProfileUpdateBody = serde_json::from_value(json!({ "bio": "Hello" })).unwrap();
        assert!(patch.audience.is_none());
        assert!(patch.show_activity.is_none());
        assert_eq!(
            serde_json::to_value(patch).unwrap(),
            json!({ "bio": "Hello" })
        );
    }

    #[test]
    fn a_person_reads_as_the_others_see_them_with_what_is_theirs_alone_beside() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b",
            "kind": "human",
            "username": "alice",
            "display_name": "Alice Chen",
            "avatar": { "text": "AC", "color": 4 },
            "email": "alice@example.org",
            "email_verified": true,
            "has_password": true,
            "totp_enabled": false,
            "email_pending": null,
            "email_code": ["sign_in", "second_step"],
            "email_code_second_step": false,
            "second_step_required": true,
            "second_step_sources": [
                "system",
                { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6d", "kind": "org", "name": "Acme", "redacted": false }
            ],
            "is_system_admin": false,
            "locale": "zh-CN",
            "timezone": "Asia/Shanghai",
            "timezone_mode": "device",
            "theme": "system",
            "density": "compact",
            "personal_org_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c",
            "created_at": "2026-10-08T17:04:05.123Z",
            "identities": [
                { "idp_key": "corp", "idp_name": "Corp", "email": null, "linked_at": "2026-10-09T08:00:00.000Z" }
            ]
        });
        let me: Me = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(me.user.username, "alice");
        assert_eq!(me.locale, Locale::ZhCn);
        assert_eq!(serde_json::to_value(&me).unwrap(), written);
        let as_others_see: UserRef = serde_json::from_value(written).unwrap();
        assert_eq!(as_others_see, me.user);
    }

    #[test]
    fn a_search_with_a_parameter_nobody_defined_is_refused() {
        let refused =
            serde_json::from_value::<UserSearchQuery>(json!({ "q": "al", "everyone": true }));
        assert!(refused.is_err());
        let accepted = serde_json::from_value::<UserSearchQuery>(json!({ "q": "al" })).unwrap();
        assert_eq!(accepted.org_id, None);
    }
}
