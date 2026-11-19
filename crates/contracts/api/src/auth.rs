//! Setup, registration, signing in by a password, by a code sent by mail and through an identity provider, the second step and what a sign-in still lacks, the change of an address, and recovery.
//!
//! A sign-in is made of proofs: a password, a code mailed to the person's
//! confirmed address, and a code of their authenticator app. It takes one
//! proof, or two different ones for a person who has a second step; what is
//! still missing is asked for by name ([`SignInProof`]). Everything that gets
//! a person in answers with an [`AuthResult`]: a short-lived access token for
//! the browser to keep in memory, beside the session cookie it was given.

use crate::operations::{
    FacetSpec as Facet, OperationSpec as Op, TargetResolver as Target, VariantSpec as Variant,
};
use orch_access_contract::key_scope::KeyAction::{Manage, Read, Write};
use orch_access_contract::key_scope::KeyIdentityKind as I;
use orch_access_contract::key_scope::KeyTargetKind::{Identity, Object};
use orch_access_contract::permissions::ObjectKind as K;

use orch_access_contract::key_scope::{KeyPlaceExclusion, KeyRule};
use orch_types::{Id, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::nodes::NodeRef;
use crate::routes::Access::{Cookie, Permission, Public, Session, User};
use crate::users::{Density, Locale, Me, Theme};

/// The fewest characters of a password. No other rule of composition applies,
/// but a password may not equal the username or the address.
pub const PASSWORD_MIN_CHARS: u32 = 10;
/// The characters of a code that is mailed.
pub const EMAIL_CODE_CHARS: u32 = 8;

/// A proof given at signing in; recovery codes replace an authenticator, never the first step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignInProof {
    /// Their password: a first step always, a second step never.
    Password,
    /// A code mailed to their confirmed address: a first step where their orgs
    /// allow it, a second step where they allow that and the person switched it on.
    EmailCode,
    /// A code of their authenticator app: a second step where they set one up.
    Totp,
    /// One unused recovery code of an enabled authenticator: a second step only.
    RecoveryCode,
}

/// Why a single sign-in through an identity provider did not succeed, as the
/// sign-in page is sent back to say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OidcCallbackError {
    /// The identity provider or the person refused.
    Denied,
    /// The sign-in took too long, or was begun elsewhere.
    StateExpired,
    /// The address is at a domain the provider's setup does not allow.
    DomainNotAllowed,
    /// Registration is closed and the person has no user yet.
    RegistrationClosed,
    /// Registration needs an invitation and none was good.
    InviteRequired,
    /// A user with the same confirmed address exists: they sign in the old way
    /// and link the identity from their profile.
    LinkRequired,
    /// The identity provider is switched off.
    Disabled,
    /// The identity provider could not be reached or answered wrongly.
    IdpError,
}

/// An identity provider, as the sign-in page offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MetaIdentityProvider {
    /// Its short key, which is part of the sign-in address.
    pub key: String,
    /// The name to show.
    pub name: String,
    /// Whether this provider may create a new account under registration policy.
    #[serde(default)]
    pub auto_register: bool,
}

/// How serious a banner is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BannerLevel {
    /// For information.
    Info,
    /// Something to be careful about.
    Warning,
}

/// A notice that the administrators put above every page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MetaBanner {
    /// What it says.
    pub text: String,
    /// How serious it is.
    pub level: BannerLevel,
    /// When it goes away by itself.
    pub until: Option<Time>,
}

/// How a connector knows the address it reaches the server at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkTrust {
    /// By the certificate of the system's own authority.
    Pinned,
    /// By a certificate any browser would trust.
    Public,
}

/// An address at which connectors reach the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MetaLinkEndpoint {
    /// The address.
    pub url: String,
    /// How it is trusted.
    pub trust: LinkTrust,
}

/// The sizes the system accepts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MetaLimits {
    /// The most characters of text in an input.
    pub max_input_chars: i64,
    /// The most bytes of an image.
    pub max_image_bytes: i64,
    /// The most bytes of a file sent through the API.
    pub max_upload_bytes: i64,
}

/// Who may register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationMode {
    /// Nobody: users are made by administrators.
    Closed,
    /// Only somebody with an invitation.
    Invite,
    /// Anybody who reaches the system.
    Open,
}

/// Who may make a team org, as the system's `orgs.created_by` says. A person's own org is made
/// with them whatever this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrgCreation {
    /// Anybody signed in.
    Anyone,
    /// System admins only.
    Admins,
    /// System admins, and somebody a system admin invited to make one; each invitation is used
    /// once.
    Invited,
}

/// What a client may know before anybody has signed in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Meta {
    /// The name of this installation.
    pub name: String,
    /// The version of the server.
    pub version: String,
    /// The build it came from.
    pub build: String,
    /// Whether the first administrator is still to be made.
    pub needs_setup: bool,
    /// Who may register.
    pub registration_mode: RegistrationMode,
    /// Who may make a team org, for the control that makes one or joins one.
    pub org_creation: OrgCreation,
    /// Whether the caller may make a team org now: never for nobody signed in; under `invited`, a
    /// system admin in a browser session, or the holder of an unused invitation to make one.
    pub may_create_org: bool,
    /// Whether the system has a mail server, so that addresses are confirmed
    /// and passwords are reset by mail.
    pub mail: bool,
    /// False when signing in by a password is for system administrators only.
    pub local_login: bool,
    /// Whether anybody here may sign in by a mailed code, which the sign-in
    /// page offers as a way; whom it lets in is a matter of each person's orgs.
    pub email_code_login: bool,
    /// The identity providers that can be signed in through.
    pub identity_providers: Vec<MetaIdentityProvider>,
    /// The languages the interface is written in.
    pub locales: Vec<Locale>,
    /// A notice to show above every page.
    pub banner: Option<MetaBanner>,
    /// The key by which browsers subscribe to push, as base64url.
    pub push_public_key: Option<String>,
    /// The addresses connectors reach the server at.
    pub link_endpoints: Vec<MetaLinkEndpoint>,
    /// The sizes the system accepts.
    pub limits: MetaLimits,
}

/// What a sign-in, a registration or a refresh answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthResult {
    /// The short-lived token to send as a bearer; kept in memory only.
    pub access_token: String,
    /// When it stops working; the client refreshes before.
    pub access_expires_at: Time,
    /// Who signed in.
    pub user: Me,
}

/// The current browser session's administrator switch and whether the person administers a place.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AdminMode {
    /// Whether this session explicitly acts as an administrator.
    pub as_admin: bool,
    /// Whether the active person has an administrative role beyond their own personal/service space, or administers the system.
    pub available: bool,
}

/// The data the signed-in browser needs once to draw its shell. Each member has the same
/// projection and access rules as its individual read, without loading any organization's tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MeBootstrapReply {
    /// The shell's public facts, with organization creation checked for this reader.
    pub meta: Meta,
    /// The signed-in person and their preferences.
    pub person: Me,
    /// Their saved interface state.
    pub state: crate::personal::PersonalState,
    /// The root organizations they may open, their own first.
    pub orgs: Vec<crate::nodes::Org>,
    /// This browser session's explicit administrator switch.
    pub admin_mode: AdminMode,
    /// Their visible starred places, in their chosen order.
    pub favorites: Vec<crate::personal::Favorite>,
    /// Their own read and following state in chats they may still read.
    pub chat_states: Vec<crate::personal::ChatState>,
    /// The person's own thread positions, exactly as the unscoped read-state read.
    #[serde(default)]
    pub read_states: Vec<crate::personal::ReadState>,
    /// The person's own explicit/automatic following choices, as the unscoped personal read.
    #[serde(default)]
    pub subscriptions: Vec<crate::personal::MeSubscription>,
    /// Their inbox counters and outside delivery status.
    pub notification_summary: crate::notifications::NotificationSummaryGetReply,
}

/// Fresh authority after personal-feed admission, without reloading preferences or Inbox state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeAuthorityReply {
    /// Current person and system standing.
    pub person: Me,
    /// The explicit switch of this browser session.
    pub admin_mode: AdminMode,
    /// Current ordinary root organizations, independent of native admin reach.
    pub orgs: Vec<crate::nodes::Org>,
    /// Public facts with organization creation evaluated for this caller.
    pub meta: Meta,
}

/// Explicitly changes only the requesting browser session's administrator switch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminModeSetBody {
    /// The requested switch; a token or an agent cannot set it.
    pub as_admin: bool,
}

/// Announces a cached private view's opening without changing any permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdminOpeningBody {
    /// A permission object kind, or chat/user; never a locator or a display name.
    pub kind: String,
    /// The opened object.
    pub id: Id,
    /// The same navigation nonce as x-orch-opening and the live subscription's opening_id.
    pub opening_id: Id,
}

/// A session of a browser, as its person lists their own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionInfo {
    /// The session's identifier.
    pub id: Id,
    /// Whether it is the one making this request.
    pub current: bool,
    /// The browser that opened it.
    pub user_agent: String,
    /// The address it was opened from, where known.
    pub ip: Option<String>,
    /// When it was opened.
    pub created_at: Time,
    /// When it was last used.
    pub last_used_at: Time,
    /// When it ends.
    pub expires_at: Time,
}

/// An API token, as it is listed: never the secret itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApiTokenInfo {
    /// The token's identifier.
    pub id: Id,
    /// The name its maker gave it.
    pub name: String,
    /// What it is allowed to do.
    pub scopes: Vec<String>,
    /// Explicit operation rules; None is a stored legacy permission-scope key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<KeyRule>>,
    /// Server-retained issuance ceilings; each rule union is required independently.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Vec<KeyRule>>,
    /// Server-retained existing refusal/lifetime ceilings, in addition to current place policy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_places: Vec<KeyPlaceExclusion>,
    /// The node it is limited to, if it is limited to one.
    pub node: Option<NodeRef>,
    /// When it was made.
    pub created_at: Time,
    /// When it was last used.
    pub last_used_at: Option<Time>,
    /// When it stops working.
    pub expires_at: Option<Time>,
}

/// A made API token, with the one chance to read its secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenCreated {
    /// The secret, in the first answer only; `null` when the same creation is asked again with its key,
    /// since no secret is kept to be given twice.
    pub token: Option<String>,
    /// The token as it is listed.
    pub info: ApiTokenInfo,
    /// The answer repeats a creation that was made already: revoke it and make another to get a secret.
    pub already_created: bool,
}

/// A committed operation acknowledgement without pre-existing resource content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenCreateReceipt {
    /// The response is an acknowledgement rather than a full resource projection.
    pub projection: crate::common::ReceiptProjection,
    /// The credential that was made.
    pub token_id: Id,
    /// The newly generated secret, once; null on an idempotent replay.
    pub token: Option<String>,
    /// When the credential was created.
    pub created_at: Time,
    /// Its accepted expiry, or null for no fixed expiry.
    pub expires_at: Option<Time>,
    /// Whether this acknowledges an earlier successful issuance.
    pub already_created: bool,
}

crate::common::full_or_receipt! {
    /// The existing full reply, or its explicit restricted receipt.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
    pub enum TokenCreateResponse {
        /// The full ordinary response.
        Full(TokenCreated),
        /// A successful outcome with no existing resource content.
        Receipt(TokenCreateReceipt),
    }
}

impl TokenCreateResponse {
    /// Preserve a fresh issuance secret while restricting pre-existing metadata on this response.
    #[must_use]
    pub fn project(created: TokenCreated, receipt_only: bool) -> Self {
        if receipt_only {
            Self::Receipt(TokenCreateReceipt {
                projection: crate::common::ReceiptProjection::Receipt,
                token_id: created.info.id,
                token: created.token,
                created_at: created.info.created_at,
                expires_at: created.info.expires_at,
                already_created: created.already_created,
            })
        } else {
            Self::full(created)
        }
    }
}

/// A fresh proof of who the caller is, given in the request that does a sensitive act: the password,
/// or for a person without one the one-use proof of their identity provider, and their second step
/// where they have one. Neither an old session nor a remembered sign-in stands for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SensitiveProof {
    /// The caller's password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// The one-use proof their identity provider gave for this act, where they have no password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_proof: Option<String>,
    /// A code of their authenticator app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<String>,
    /// A code mailed to their confirmed address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_code: Option<String>,
    /// One unused recovery code, instead of a current authenticator code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_code: Option<String>,
}

/// One safe action/target from the shared operation resolver. Historical target names are never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApiTokenActivityFacet {
    /// Canonical shared operation/variant.
    pub operation: String,
    /// Existing business action, not inferred from a method or route.
    pub action: orch_access_contract::key_scope::KeyAction,
    /// Actual business resource family.
    pub resource_kind: orch_access_contract::key_scope::KeyTargetKind,
    /// Safe governing identity; no request path, query or content.
    pub target: orch_access_contract::key_scope::KeyObjectRef,
    /// Its name only where the reader's current ordinary access permits knowing it.
    pub target_name: Option<String>,
}

/// A bounded recent authenticated key call; the existing audit trail remains separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ApiTokenActivity {
    /// Record identity.
    pub id: Id,
    /// Server call time.
    pub at: Time,
    /// Stable entry operation ID.
    pub operation: String,
    /// Server-selected canonical variant, absent when validation never resolved one.
    pub selection: Option<String>,
    /// Exact safe action/target metadata; an org reader receives only that org's slice.
    pub facets: Vec<ApiTokenActivityFacet>,
    /// Declared HTTP/protocol method.
    pub method: String,
    /// Declared route template or protocol label; never a raw URL.
    pub route: String,
    /// Exact response or protocol status, including socket closure status.
    pub status: u16,
    /// Stable outcome/error code only, never a message or response body.
    pub error_code: Option<String>,
    /// Marked only by an actual replay producer.
    pub replayed: bool,
    /// Monotonic duration through the known outcome.
    pub duration_ms: u64,
    /// Existing request identity.
    pub request_id: String,
    /// Source IP under the existing account/audit privacy rules.
    pub ip: Option<String>,
}

/// Pages at most the most recent hundred calls, with nothing older than seven days.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiTokenActivityQuery {
    /// At most one hundred; ordinary page size when omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Bound recent-list snapshot and timestamp/identity cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// A token that can reach an org, as the org's managers see it: never its secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OrgToken {
    /// The token as it is listed.
    #[serde(flatten)]
    pub info: ApiTokenInfo,
    /// Whose it is.
    pub user: crate::users::UserRef,
    /// It belongs to a service identity.
    pub service: bool,
    /// It was refused in this org.
    pub refused_here: bool,
    /// Its whole life is within the longest this org admits.
    pub lifetime_allowed: bool,
}

/// Makes the first system administrator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetupCreateBody {
    /// The username to sign in with.
    pub username: String,
    /// The name to show.
    pub display_name: String,
    /// The password, at least [`PASSWORD_MIN_CHARS`] characters.
    pub password: String,
    /// Their address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The browser's time zone, an IANA name: their zone follows their device from the start, and is
    /// UTC until a browser reports one where none is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// Registers a new user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthRegisterBody {
    /// The username to sign in with.
    pub username: String,
    /// The name to show.
    pub display_name: String,
    /// The password, at least [`PASSWORD_MIN_CHARS`] characters.
    pub password: String,
    /// Their address; a personal invitation requires the address it was made for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The secret of an invitation, where registration needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitation: Option<String>,
    /// Explicitly accept the invitation's target as part of this registration/sign-in.
    /// Standalone registration uses a system admission link and joins no team.
    #[serde(default)]
    pub accept_invitation: bool,
    /// The browser's time zone, an IANA name: their zone follows their device from the start, and is
    /// UTC until a browser reports one where none is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

/// The answer of a registration that has to be confirmed first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationRequired {
    /// Always true: registration continues through the mailbox, without a browser session.
    #[serde(deserialize_with = "verification_is_required")]
    #[schemars(extend("const" = true))]
    pub verification_required: bool,
}

fn verification_is_required<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<bool, D::Error> {
    if bool::deserialize(deserializer)? {
        Ok(true)
    } else {
        Err(serde::de::Error::custom(
            "verification_required must be true",
        ))
    }
}

/// Checks only the public namespace shared by usernames and root-org slugs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthNameAvailabilityGetQuery {
    /// A prospective username or root-org slug; never an email address.
    pub name: String,
}

/// Whether a public name is currently free; creation still checks it atomically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthNameAvailabilityGetReply {
    /// False when a username or root-org slug already holds this name.
    pub available: bool,
}

/// The account created by registration and the result of any explicitly composed acceptance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthRegisterResult {
    /// The ordinary sign-in result, kept independent of invitations.
    #[serde(flatten)]
    pub auth: AuthResult,
    /// The accepted destination, filtered through the new session's current access.
    #[serde(default)]
    pub accepted_invitation: Option<Box<crate::users::InvitationAcceptReply>>,
}

/// How a registration was answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[expect(
    clippy::large_enum_variant,
    reason = "the answer is built once to be written; boxing the session would put a layer in every use"
)]
pub enum AuthRegisterReply {
    /// `201`: the user is made and signed in.
    Registered(AuthRegisterResult),
    /// `202`: continue through the mailbox before signing in; no session is created here.
    Pending(VerificationRequired),
}

/// Signs a person in with one proof, or with two for somebody who has a second step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthLoginBody {
    /// A username or an address.
    pub login: String,
    /// Their password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// A code that was mailed to them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_code: Option<String>,
    /// A code of their authenticator app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<String>,
    /// One unused backup code of their authenticator, instead of its current code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_code: Option<String>,
}

/// Asks for a sign-in code to be mailed. It is answered the same whatever
/// the name is, so that it tells nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthEmailCodeSendBody {
    /// A username or an address.
    pub login: String,
    /// Their password, which makes a code a second step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

/// Asks for a link to reset a password.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthRecoveryStartBody {
    /// The address of the user.
    pub email: String,
}

/// Sets a new password with the token of a reset link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthRecoveryConfirmBody {
    /// The secret of the link.
    pub token: String,
    /// The new password.
    pub password: String,
}

/// Confirms an address with the token of the link that was mailed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthEmailVerifyBody {
    /// The secret of the link.
    pub token: String,
}

/// Asks for the confirmation link to be mailed again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthEmailVerifyResendBody {
    /// A username or an address.
    pub login: String,
}

/// Changes what a person set for themself. An absent member stays as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeUpdateBody {
    /// The name to show.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The identity palette index, 0 through 11; null restores the default color.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<u16>")]
    pub avatar_color: Option<Option<u16>>,
    /// The language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<Locale>,
    /// The time zone, as an IANA name. Given without `timezone_mode`, it is a zone the person chose,
    /// and the mode becomes `fixed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Whether the zone follows the device or is the one chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone_mode: Option<crate::users::TimezoneMode>,
    /// True on a browser's own report of its zone (with `timezone` and `timezone_mode` `device`): it is
    /// written only while the person's zone still follows their device, so that a report that arrives
    /// after they chose a zone changes nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone_report: Option<bool>,
    /// The colour scheme.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<Theme>,
    /// How closely the interface is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density: Option<Density>,
    /// A new address, pending confirmation, or null to clear it while preserving sign-in.
    /// An actual change asks for the current password where there is one.
    #[serde(
        default,
        deserialize_with = "crate::common::absent_or_null",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<String>")]
    pub email: Option<Option<String>>,
    /// The password, when a new address is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_password: Option<String>,
}

/// Where to go on to sign in through an identity provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthOidcStartQuery {
    /// An in-app path to return to afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
    /// The secret of an invitation, where registration needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invitation: Option<String>,
    /// Explicitly accept the invitation's target as part of this registration/sign-in.
    /// Standalone registration uses a system admission link and joins no team.
    #[serde(default)]
    pub accept_invitation: bool,
    /// From a browser that is signed in: the provider proves its person again for this one act,
    /// and the callback gives a one-use proof instead of a session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sensitive_purpose: Option<SensitivePurpose>,
    /// The service identity whose token the proof is for, with `token_create`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_id: Option<Id>,
}

/// What an identity provider sends back to the callback. The provider may add
/// parameters of its own, and says it refused by sending an error and no code,
/// so this query neither requires nor refuses anything: the handler reads the
/// rest from the raw query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AuthOidcCallbackQuery {
    /// The code to exchange for the person's identity; absent when the
    /// provider refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The state the sign-in began with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    /// What the provider reports when it did not sign the person in (`access_denied`, or another).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The provider's own words for it; for the log, never for the person.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_description: Option<String>,
}

/// Where the identity provider is reached to link an identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeIdentityLinkReply {
    /// The address to send the browser to.
    pub redirect_url: String,
}

/// Changes the password of the signed-in person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MePasswordChangeBody {
    /// Their current password for a browser or legacy credential; an admitted explicit key authenticates the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_password: Option<String>,
    /// The password to have.
    pub new_password: String,
}

/// Begins the setup of an authenticator app: with the password, or, for a person who has none, the
/// one-use proof their identity provider gave for setting up a second step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeTotpSetupBody {
    /// Their password, where they have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// The one-use proof of their identity provider, where they have no password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_proof: Option<String>,
}

/// Gives this browser session's second proof again: one code of the second step the person has.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeSecondStepVerifyBody {
    /// A code of their authenticator app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totp: Option<String>,
    /// A code mailed to their confirmed address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email_code: Option<String>,
    /// One unused recovery code of their authenticator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_code: Option<String>,
}

/// What a proof of the identity provider is asked for, when a person without a password proves who
/// they are again for one sensitive act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SensitivePurpose {
    /// Making an API token, theirs or a service identity's.
    TokenCreate,
    /// Setting up a second step.
    SecondStepSetup,
    /// Replacing the one-use recovery codes of an enabled authenticator.
    RecoveryCodes,
}

/// The secret an authenticator app is set up with. Asking again before it is
/// switched on answers with the same pending secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MeTotpSetupReply {
    /// The secret.
    pub secret: String,
    /// The same as an address an app can read from a picture.
    pub otpauth_url: String,
}

/// Switches the authenticator app on, with a code it shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeTotpEnableBody {
    /// A code of the app.
    pub code: String,
}

/// Metadata for managing an enabled authenticator's backups; never the codes or their hashes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecoveryCodeStatus {
    /// How many saved codes remain unused.
    pub remaining: i64,
    /// When the current batch was issued, if one exists.
    pub created_at: Option<Time>,
    /// The second proofs that can authorize a replacement now, beside a fresh first proof.
    pub methods: Vec<SignInProof>,
    /// This session just verified a recovery code and may replace the batch once with a fresh first proof.
    pub recent_recovery: bool,
}

/// Plaintext backups returned only by issuance or regeneration. Ordinary reads never return them.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RecoveryCodeBatch {
    /// Ten one-use codes; save them before closing the reply.
    pub codes: Vec<String>,
    /// Safe metadata for updating the management dialog without another request.
    pub status: RecoveryCodeStatus,
}

impl std::fmt::Debug for RecoveryCodeBatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RecoveryCodeBatch")
            .field("code_count", &self.codes.len())
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// Authenticator activation and its one-time backup codes, with the current account for the screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MeTotpEnableReply {
    /// The account after activation; no second request is needed to update the screen.
    pub me: crate::users::Me,
    /// Shown only by this successful activation reply.
    pub recovery_codes: RecoveryCodeBatch,
}

/// Freshly proves the person before replacing every old recovery code.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeRecoveryCodesRegenerateBody {
    /// Their password or scoped identity-provider proof, with their second step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<SensitiveProof>,
}

/// Switches the authenticator app off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeTotpDisableBody {
    /// Their password for a browser or legacy credential; unnecessary for an admitted explicit key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// A code of the app, or one unused saved recovery code.
    pub code: String,
}

/// Asks for a code to be mailed to the confirmed address: to make it a second step, with the password
/// or, for a person who has none, the one-use proof of their identity provider; or, with neither, to
/// give this session's second proof where a mailed code is their second step already.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeEmailCodeSendBody {
    /// Their password, where they have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// The one-use proof of their identity provider, where they have no password.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_proof: Option<String>,
}

/// Makes a mailed code a second step of the person, with a code just mailed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeEmailCodeEnableBody {
    /// The code that was mailed.
    pub code: String,
}

/// Stops a mailed code being a second step of the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MeEmailCodeDisableBody {
    /// Their password for a browser or legacy credential; unnecessary for an admitted explicit key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// A mailed code, or one of their authenticator.
    pub code: String,
}

/// Makes an API token, for the signed-in person or for a service identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiTokenCreateBody {
    /// A name to tell it by.
    pub name: String,
    /// The permissions it may use, by name, or `*` for everything its person may; left out, it may
    /// use none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// Explicit operation/action/target rules, already expanded from a preset. Cannot be mixed
    /// with legacy scopes/node_id. Omitting both modes creates an operation-scoped deny-all key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rules: Option<Vec<KeyRule>>,
    /// Limits it to one node and what is inside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<Id>,
    /// When it stops working; never, when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<Time>,
    /// The browser maker's existing fresh proof. An explicitly admitted scoped-key act is
    /// authenticated by that credential and does not submit another browser/password proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof: Option<SensitiveProof>,
}

crate::endpoints! {
    /// Answers `ok` when the API can reach its database.
    HealthGet: GET "/health", Public, none, none, raw, 200,
        operation Op::public();
    /// Tells a client what it may know before anybody has signed in.
    MetaGet: GET "/meta", Public, none, none, json Meta, 200,
        operation Op::public();
    /// Makes the first system administrator, while the system still needs one.
    SetupCreate: POST "/setup", Public, none, json SetupCreateBody, json AuthResult, 200,
        operation Op::protocol();
    /// Registers a user: answers `201` with a session, or `202` when the address is to be confirmed first.
    AuthRegister: POST "/auth/register", Public, none, json AuthRegisterBody, json AuthRegisterReply, 201,
        responses [201 => AuthRegisterResult, 202 => VerificationRequired],
        operation Op::protocol();
    /// Checks whether a public username or root-org slug is free, without accepting an email address.
    AuthNameAvailabilityGet: GET "/auth/name-availability", Public, query AuthNameAvailabilityGetQuery, none, json AuthNameAvailabilityGetReply, 200,
        operation Op::protocol();
    /// Signs a person in with one proof, or two where they have a second step.
    AuthLogin: POST "/auth/login", Public, none, json AuthLoginBody, json AuthResult, 200,
        operation Op::protocol();
    /// Mails a sign-in code where one may be sent; answers `202` whatever the name.
    AuthEmailCodeSend: POST "/auth/email-code", Public, none, json AuthEmailCodeSendBody, none, 202,
        operation Op::protocol();
    /// Gives a new access token to the browser that holds the session cookie.
    AuthRefresh: POST "/auth/refresh", Cookie, none, none, json AuthResult, 200,
        operation Op::protocol();
    /// Ends the session of the browser and revokes it.
    AuthLogout: POST "/auth/logout", Cookie, none, none, none, 204,
        operation Op::protocol();
    /// Mails a link to reset a password; answers `202` whatever the address.
    AuthRecoveryStart: POST "/auth/recovery", Public, none, json AuthRecoveryStartBody, none, 202,
        operation Op::protocol();
    /// Checks that a reset link is still usable without consuming it or revealing its account.
    AuthRecoveryCheck: GET "/auth/recovery/{token}", Public, none, none, none, 204,
        operation Op::protocol();
    /// Sets a new password with the token of a reset link and revokes every session of the user.
    AuthRecoveryConfirm: POST "/auth/recovery/confirm", Public, none, json AuthRecoveryConfirmBody, none, 204,
        operation Op::protocol();
    /// Confirms an address with the token of the link that was mailed.
    AuthEmailVerify: POST "/auth/verify-email", Public, none, json AuthEmailVerifyBody, none, 204,
        operation Op::protocol();
    /// Mails the confirmation link again; answers `202` whatever the name.
    AuthEmailVerifyResend: POST "/auth/verify-email/resend", Public, none, json AuthEmailVerifyResendBody, none, 202,
        operation Op::protocol();
    /// Reads the signed-in person.
    MeGet: GET "/me", User, none, none, json Me, 200,
        operation Op::single(Read, &[Identity(I::Account)], Target::CurrentAccount);
    /// Loads the browser shell's personal data in one request; trees and chat contents stay lazy.
    MeBootstrapGet: GET "/me/bootstrap", Session, none, none, json MeBootstrapReply, 200,
        operation Op::facets(&[
            Facet::required(Read, &[Identity(I::Account)], Target::CurrentAccount),
            Facet::response_content(&[Object(K::Org)], Target::ResolvedCollection("account_organizations")),
            Facet::response_content(&[Object(K::Org), Object(K::Project), Object(K::Topic), Object(K::Thread), Object(K::Document), Object(K::DocumentContainer), Object(K::Provider), Object(K::Agent), Object(K::Environment), Object(K::Host), Object(K::Machine), Object(K::Template), Object(K::McpServer), Object(K::Mount), Object(K::Skill)], Target::ResolvedCollection(crate::account_sources::ORGANIZATION_CONTEXT_ROLE)),
            Facet::response_content(&[Object(K::Org), Object(K::Project), Object(K::Topic), Object(K::Thread)], Target::ResolvedCollection("favorite_nodes")),
            Facet::response_content(&[Object(K::Org)], Target::ResolvedCollection("favorite_names_orgs")),
            Facet::response_content(&[Object(K::Project)], Target::ResolvedCollection("favorite_names_projects")),
            Facet::response_content(&[Object(K::Topic)], Target::ResolvedCollection("favorite_names_topics")),
            Facet::response_content(&[Object(K::Thread)], Target::ResolvedCollection("favorite_names_threads")),
            Facet::response_content(&[Object(K::Thread)], Target::ResolvedCollection("read_state_threads")),
            Facet::response_content(&[Object(K::Org), Object(K::Project), Object(K::Topic), Object(K::Thread)], Target::ResolvedCollection("subscription_nodes")),
            Facet::response_content(&[Identity(I::Chat)], Target::ResolvedCollection("personal_state_chats")),
        ]);
    /// Retakes current person, session mode, organizations and Meta after a feed is admitted.
    MeAuthorityGet: GET "/me/authority", Session, none, none, json MeAuthorityReply, 200,
        operation Op::facets(&[
            Facet::required(Read, &[Identity(I::Account)], Target::CurrentAccount),
            Facet::response_content(&[Object(K::Org)], Target::ResolvedCollection("account_organizations")),
            Facet::response_content(&[Object(K::Org), Object(K::Project), Object(K::Topic), Object(K::Thread), Object(K::Document), Object(K::DocumentContainer), Object(K::Provider), Object(K::Agent), Object(K::Environment), Object(K::Host), Object(K::Machine), Object(K::Template), Object(K::McpServer), Object(K::Mount), Object(K::Skill)], Target::ResolvedCollection(crate::account_sources::ORGANIZATION_CONTEXT_ROLE)),
        ]);
    /// Changes what the signed-in person set for themself, an address included.
    MeUpdate: PATCH "/me", User, none, json MeUpdateBody, json Me, 200,
        operation Op::variants(&[
            Variant {
                name: "profile",
                selector: "profile or interface fields are present and email is absent",
                facets: &[
                    Facet::required(Write, &[Identity(I::Account)], Target::CurrentAccount),
                    Facet::response_content(&[Identity(I::Account)], Target::CurrentAccount),
                ],
            },
            Variant {
                name: "security",
                selector: "email is present, including explicit null, and profile or interface fields are absent",
                facets: &[
                    Facet::required(Manage, &[Identity(I::Account)], Target::CurrentAccount),
                    Facet::response_content(&[Identity(I::Account)], Target::CurrentAccount),
                ],
            },
            Variant {
                name: "profile_and_security",
                selector: "email and profile or interface fields are both present",
                facets: &[
                    Facet::required(Write, &[Identity(I::Account)], Target::CurrentAccount),
                    Facet::required(Manage, &[Identity(I::Account)], Target::CurrentAccount),
                    Facet::response_content(&[Identity(I::Account)], Target::CurrentAccount),
                ],
            },
        ]);
    /// Sends the browser to an identity provider to sign in.
    AuthOidcStart: GET "/auth/oidc/{key}/start", Public, query AuthOidcStartQuery, none, raw, 302,
        operation Op::protocol();
    /// Finishes a sign-in through an identity provider and sets the session cookie.
    AuthOidcCallback: GET "/auth/oidc/{key}/callback", Public, query AuthOidcCallbackQuery, none, raw, 302,
        operation Op::protocol();
    /// Begins linking an identity provider to the signed-in user.
    MeIdentityLink: POST "/me/identities/{key}/link", User, none, none, json MeIdentityLinkReply, 200,
        operation Op::single(Manage, &[Identity(I::Account)], Target::CurrentAccount);
    /// Unlinks an identity, unless that would leave no way to sign in.
    MeIdentityDelete: DELETE "/me/identities/{key}", User, none, none, none, 204,
        operation Op::single(Manage, &[Identity(I::Account)], Target::CurrentAccount);
    /// Changes the password and revokes the other sessions.
    MePasswordChange: POST "/me/password", User, none, json MePasswordChangeBody, none, 204,
        operation Op::single(Manage, &[Identity(I::Account)], Target::CurrentAccount);
    /// Begins the setup of an authenticator app and returns its secret.
    MeTotpSetup: POST "/me/totp/setup", User, none, json MeTotpSetupBody, json MeTotpSetupReply, 200,
        operation Op::single(Manage, &[Identity(I::Account)], Target::CurrentAccount);
    /// Switches the authenticator app on.
    MeTotpEnable: POST "/me/totp/enable", User, none, json MeTotpEnableBody, json MeTotpEnableReply, 200,
        operation Op::single(Manage, &[Identity(I::Account)], Target::CurrentAccount);
    /// Reads only backup-code metadata and available second proofs, never saved code values.
    MeRecoveryCodesGet: GET "/me/recovery-codes", Session, none, none, json RecoveryCodeStatus, 200,
        operation Op::single(Read, &[Identity(I::Account)], Target::CurrentAccount);
    /// Replaces every older authenticator recovery code; plaintext is returned once, never read later.
    MeRecoveryCodesRegenerate: POST "/me/recovery-codes", Session, none, json MeRecoveryCodesRegenerateBody, json RecoveryCodeBatch, 200,
        operation Op::single(Manage, &[Identity(I::Account)], Target::CurrentAccount);
    /// Switches the authenticator app off.
    MeTotpDisable: POST "/me/totp/disable", User, none, json MeTotpDisableBody, none, 204,
        operation Op::single(Manage, &[Identity(I::Account)], Target::CurrentAccount);
    /// Mails a code to the confirmed address, to make it a second step.
    MeEmailCodeSend: POST "/me/email-code/send", User, none, json MeEmailCodeSendBody, none, 202,
        operation Op::single(Manage, &[Identity(I::Account)], Target::CurrentAccount);
    /// Makes a mailed code a second step of the signed-in person.
    MeEmailCodeEnable: POST "/me/email-code/enable", User, none, json MeEmailCodeEnableBody, none, 204,
        operation Op::single(Manage, &[Identity(I::Account)], Target::CurrentAccount);
    /// Stops a mailed code being a second step.
    MeEmailCodeDisable: POST "/me/email-code/disable", User, none, json MeEmailCodeDisableBody, none, 204,
        operation Op::single(Manage, &[Identity(I::Account)], Target::CurrentAccount);
    /// Gives this browser session's second proof, which the orgs that require one wait for.
    MeSecondStepVerify: POST "/me/second-step/verify", Session, none, json MeSecondStepVerifyBody, json Me, 200,
        operation Op::protocol();
    /// Reads this browser session's explicit administrator mode.
    MeAdminModeGet: GET "/me/admin-mode", Session, none, none, json AdminMode, 200,
        operation Op::browser_elevation();
    /// Changes this browser session's explicit administrator mode; enabling needs current administration standing.
    MeAdminModeSet: PUT "/me/admin-mode", Session, none, json AdminModeSetBody, json AdminMode, 200,
        operation Op::browser_elevation();
    /// Rechecks and records an opening of a cached private view using this session's native reach.
    MeAdminOpening: POST "/me/admin-openings", Session, none, json AdminOpeningBody, none, 204,
        operation Op::browser_elevation();
    /// Lists the sessions of the signed-in person.
    /// Reading the account collection requires its Read scope; each returned credential also needs this operation's own Read scope. Account self alone does not reveal its credentials.
    MeSessionList: GET "/me/sessions", User, none, none, json Vec<SessionInfo>, 200,
        operation Op::facets(&[
            Facet::required(Read, &[Identity(I::Session)], Target::CurrentAccount),
            Facet::response_content(&[Identity(I::Session)], Target::ResolvedCollection("listed_sessions")),
        ]);
    /// Signs the person out of every other browser, retaining exactly the session that makes this request.
    MeSessionOthersDelete: DELETE "/me/sessions", Session, none, none, json Vec<SessionInfo>, 200,
        operation Op::facets(&[
            Facet::required(Manage, &[Identity(I::Session)], Target::ResolvedCollection("selected_sessions")),
            Facet::response_content(&[Identity(I::Session)], Target::ResolvedCollection("remaining_sessions")),
        ]);
    /// Ends one of the sessions of the signed-in person.
    MeSessionDelete: DELETE "/me/sessions/{id}", User, none, none, none, 204,
        operation Op::legacy_tokens(&[], &Op::single(Manage, &[Identity(I::Session)], Target::Path("id")));
    /// Lists the API tokens of the signed-in person.
    /// Reading the account collection requires its Read scope; each returned credential also needs this operation's own Read scope. Account self alone does not reveal its credentials.
    MeTokenList: GET "/me/tokens", User, none, none, json Vec<ApiTokenInfo>, 200,
        operation Op::facets(&[
            Facet::required(Read, &[Identity(I::Key)], Target::CurrentAccount),
            Facet::response_content(&[Identity(I::Key)], Target::ResolvedCollection("listed_keys")),
            Facet::response_content(&[Object(K::Org)], Target::ResolvedCollection("key_scope_orgs")),
            Facet::response_content(&[Object(K::Project)], Target::ResolvedCollection("key_scope_projects")),
            Facet::response_content(&[Object(K::Topic)], Target::ResolvedCollection("key_scope_topics")),
            Facet::response_content(&[Object(K::Thread)], Target::ResolvedCollection("key_scope_threads")),
        ]);
    /// Reads this account's owned key history, including authenticated refusals and replays.
    MeTokenActivityList: GET "/me/tokens/{id}/activity", User, query ApiTokenActivityQuery, none, json crate::common::Page<ApiTokenActivity>, 200,
        operation Op::single(Read, &[Identity(I::Key)], Target::Resolved("owned_key"));
    /// Reads only the selected organization's recorded activity facets under current membership management.
    OrgTokenActivityList: GET "/orgs/{id}/tokens/{token_id}/activity", Permission("members.manage"), query ApiTokenActivityQuery, none, json crate::common::Page<ApiTokenActivity>, 200,
        operation Op::single(Read, &[Identity(I::Key)], Target::Resolved("activity_org"));
    /// Makes a key through the shared authorized account operation; its secret is shown once.
    MeTokenCreate: POST "/me/tokens", Session, none, json ApiTokenCreateBody, json TokenCreateResponse, 200,
        operation Op::receipt(&Op::returning(Manage, &[Identity(I::Key)], Target::CurrentAccount));
    /// Revokes an API token.
    MeTokenDelete: DELETE "/me/tokens/{id}", User, none, none, none, 204,
        operation Op::returning(Manage, &[Identity(I::Key)], Target::Resolved("owned_key"));
    /// Lists the tokens that can reach anything in an org, with whether each is refused here or lives too long for it.
    /// Organization governance admits the inventory operation; each full credential additionally
    /// needs this operation's own Read on its real Key/owning Account, not an invented org placement.
    OrgTokenList: GET "/orgs/{id}/tokens", Permission("members.manage"), query OrgTokenListQuery, none, json crate::common::Page<OrgToken>, 200,
        operation Op::facets(&[
            Facet::required(Manage, &[Identity(I::Key)], Target::Resolved("org_keys")),
            Facet::response_content(&[Identity(I::Key)], Target::ResolvedCollection("listed_keys")),
            Facet::response_content(&[Object(K::Org)], Target::ResolvedCollection("key_scope_orgs")),
            Facet::response_content(&[Object(K::Project)], Target::ResolvedCollection("key_scope_projects")),
            Facet::response_content(&[Object(K::Topic)], Target::ResolvedCollection("key_scope_topics")),
            Facet::response_content(&[Object(K::Thread)], Target::ResolvedCollection("key_scope_threads")),
        ]);
    /// Refuses a token in an org and everything inside it, without revoking it elsewhere.
    /// Its empty acknowledgement remains available with Manage; names in an ordinary
    /// refusal additionally follow this operation's Read on the actual governing org.
    OrgTokenRefuse: POST "/orgs/{id}/tokens/{token_id}/refuse", Permission("members.manage"), none, none, none, 204,
        operation Op::facets(&[
            Facet::required(Manage, &[Identity(I::Key)], Target::Resolved("org_key")),
            Facet::response_content(&[Object(K::Org)], Target::Resolved("refusal_org")),
        ]);
}

/// Pages the tokens that reach an org.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgTokenListQuery {
    /// How many at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Where the page begins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_token_is_asked_for_with_a_fresh_proof_and_its_secret_is_given_once() {
        let asked: ApiTokenCreateBody = serde_json::from_value(json!({
            "name": "deploy", "scopes": ["thread.view"],
            "proof": { "password": "my own long phrase", "totp": "123456" }
        }))
        .unwrap();
        assert_eq!(
            asked.proof.as_ref().unwrap().totp.as_deref(),
            Some("123456")
        );
        assert_eq!(asked.proof.as_ref().unwrap().identity_proof, None);
        let key_request: ApiTokenCreateBody =
            serde_json::from_value(json!({ "name": "deploy", "rules": [] })).unwrap();
        assert!(
            key_request.proof.is_none(),
            "scoped-key authentication supplies no browser proof; browser enforcement remains in the shared producer"
        );
        let info = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "name": "deploy", "scopes": ["thread.view"],
            "node": null, "created_at": "2026-10-08T17:04:05.123Z", "last_used_at": null, "expires_at": null
        });
        let again = json!({ "token": null, "info": info, "already_created": true });
        let read: TokenCreated = serde_json::from_value(again.clone()).unwrap();
        assert!(read.already_created && read.token.is_none());
        assert_eq!(serde_json::to_value(&read).unwrap(), again);
    }

    #[test]
    fn a_registration_that_waits_for_the_address_is_told_from_one_that_signed_in() {
        let waiting: AuthRegisterReply =
            serde_json::from_value(json!({ "verification_required": true })).unwrap();
        assert_eq!(
            waiting,
            AuthRegisterReply::Pending(VerificationRequired {
                verification_required: true
            })
        );
        assert_eq!(
            serde_json::to_value(&waiting).unwrap(),
            json!({ "verification_required": true })
        );
        assert!(
            serde_json::from_value::<AuthRegisterReply>(json!({ "verification_required": false }))
                .is_err()
        );
    }

    #[test]
    fn an_omitted_address_or_color_is_preserved_and_null_requests_a_clear() {
        let omitted: MeUpdateBody = serde_json::from_value(json!({})).unwrap();
        assert_eq!(omitted.email, None);
        assert_eq!(omitted.avatar_color, None);
        assert_eq!(serde_json::to_value(&omitted).unwrap(), json!({}));
        let clear: MeUpdateBody =
            serde_json::from_value(json!({ "email": null, "avatar_color": null })).unwrap();
        assert_eq!(clear.email, Some(None));
        assert_eq!(clear.avatar_color, Some(None));
        assert_eq!(
            serde_json::to_value(&clear).unwrap(),
            json!({ "email": null, "avatar_color": null })
        );
        let set: MeUpdateBody =
            serde_json::from_value(json!({ "email": "person@example.org", "avatar_color": 11 }))
                .unwrap();
        assert_eq!(
            set.email.as_ref().and_then(Option::as_deref),
            Some("person@example.org")
        );
        assert_eq!(set.avatar_color, Some(Some(11)));
    }

    #[test]
    fn a_banner_without_an_end_is_written_with_null() {
        let meta: MetaBanner = serde_json::from_value(
            json!({ "text": "Maintenance", "level": "warning", "until": null }),
        )
        .unwrap();
        assert_eq!(meta.level, BannerLevel::Warning);
        assert_eq!(
            serde_json::to_value(&meta).unwrap()["until"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn a_sign_in_names_its_proofs_and_nothing_else() {
        let body: AuthLoginBody = serde_json::from_value(
            json!({ "login": "alice", "password": "correct horse", "totp": "123456" }),
        )
        .unwrap();
        assert_eq!(body.email_code, None);
        assert!(
            serde_json::from_value::<AuthLoginBody>(json!({ "login": "alice", "remember": true }))
                .is_err()
        );
    }

    #[test]
    fn a_refusal_from_the_identity_provider_still_reaches_the_callback() {
        let refused: AuthOidcCallbackQuery =
            serde_json::from_value(json!({ "state": "s", "error": "access_denied" })).unwrap();
        assert_eq!(refused.code, None);
        assert_eq!(refused.state.as_deref(), Some("s"));
    }

    #[test]
    fn a_second_step_is_set_up_with_the_password_or_with_a_proof_of_the_identity_provider() {
        let with_password: MeTotpSetupBody =
            serde_json::from_value(json!({ "password": "my own long phrase" })).unwrap();
        assert_eq!(with_password.identity_proof, None);
        let without: MeEmailCodeSendBody =
            serde_json::from_value(json!({ "identity_proof": "oip_abc" })).unwrap();
        assert_eq!(without.identity_proof.as_deref(), Some("oip_abc"));
        assert_eq!(
            serde_json::to_value(MeEmailCodeSendBody::default()).unwrap(),
            json!({})
        );
        let verify: MeSecondStepVerifyBody =
            serde_json::from_value(json!({ "totp": "123456" })).unwrap();
        assert_eq!(verify.email_code, None);
        let start: AuthOidcStartQuery = serde_json::from_value(
            json!({ "redirect": "/settings/tokens", "sensitive_purpose": "second_step_setup" }),
        )
        .unwrap();
        assert_eq!(
            start.sensitive_purpose,
            Some(SensitivePurpose::SecondStepSetup)
        );
        assert!(
            serde_json::from_value::<AuthOidcStartQuery>(
                json!({ "sensitive_purpose": "anything" })
            )
            .is_err()
        );
    }

    #[test]
    fn a_proof_is_written_as_the_sign_in_asks_for_it() {
        assert_eq!(
            serde_json::to_value([
                SignInProof::Password,
                SignInProof::EmailCode,
                SignInProof::Totp,
                SignInProof::RecoveryCode
            ])
            .unwrap(),
            json!(["password", "email_code", "totp", "recovery_code"])
        );
    }

    #[test]
    fn recovery_is_a_named_optional_second_proof_and_replacement_has_a_scoped_purpose() {
        let literal = json!({"login":"ada", "password":"a long phrase", "recovery_code":"ABCD-EFGH-JKMN-NPQR-STVW"});
        let body: AuthLoginBody = serde_json::from_value(literal.clone()).unwrap();
        assert_eq!(serde_json::to_value(body).unwrap(), literal);
        let verify: MeSecondStepVerifyBody =
            serde_json::from_value(json!({"recovery_code":"saved"})).unwrap();
        assert_eq!(verify.totp, None);
        let replace: MeRecoveryCodesRegenerateBody =
            serde_json::from_value(json!({"proof":{"password":"long", "recovery_code":"saved"}}))
                .unwrap();
        assert_eq!(
            replace
                .proof
                .as_ref()
                .and_then(|proof| proof.recovery_code.as_deref()),
            Some("saved")
        );
        let start: AuthOidcStartQuery =
            serde_json::from_value(json!({"sensitive_purpose":"recovery_codes"})).unwrap();
        assert_eq!(
            start.sensitive_purpose,
            Some(SensitivePurpose::RecoveryCodes)
        );
    }

    #[test]
    fn token_receipts_preserve_the_new_secret_and_hide_existing_metadata_on_replay() {
        let info = json!({
            "id":"00000000-0000-0000-0000-000000000001", "name":"Private earlier key name",
            "scopes":["doc.view"],
            "node":{"id":"00000000-0000-0000-0000-000000000002", "kind":"project", "name":"Private project name", "redacted":false},
            "created_at":"2026-11-01T00:00:00Z", "last_used_at":null, "expires_at":null
        });
        for (secret, replay) in [(Some("new-secret-once"), false), (None, true)] {
            let full: TokenCreated = serde_json::from_value(json!({
                "token":secret, "info":info, "already_created":replay
            }))
            .unwrap();
            let original = serde_json::to_value(&full).unwrap();
            assert_eq!(
                serde_json::to_value(TokenCreateResponse::project(full.clone(), false)).unwrap(),
                original
            );
            let receipt = serde_json::to_value(TokenCreateResponse::project(full, true)).unwrap();
            assert_eq!(receipt["projection"], "receipt");
            assert_eq!(receipt["token"], json!(secret));
            assert_eq!(receipt["already_created"], replay);
            assert!(receipt.get("info").is_none());
            assert!(!receipt.to_string().contains("Private"));
            assert!(matches!(
                serde_json::from_value::<TokenCreateResponse>(receipt).unwrap(),
                TokenCreateResponse::Receipt(_)
            ));
        }
    }
}
