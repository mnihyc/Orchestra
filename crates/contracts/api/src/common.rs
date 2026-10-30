//! Pagination, errors, identifiers and HTTP conventions.
//!
//! The base path is [`BASE_PATH`]. Bodies are JSON with `snake_case`
//! members. A request with a member nobody defined is refused; a client
//! ignores members it does not know in a response. A member that may be
//! `null` is always present; a member that may be absent is absent, never
//! `null`, unless it is documented as both.

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

pub use orch_types::{Hash, Id, Revision, Seq, Time};

/// Where every route lives.
pub const BASE_PATH: &str = "/api/v1";
/// The header every response carries, and an error repeats in its body.
pub const REQUEST_ID_HEADER: &str = "x-request-id";
/// The header a request that is authenticated by a cookie alone must carry.
pub const CSRF_HEADER: &str = "x-orch-csrf";
/// The header by which a creating request is made safe to repeat.
pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";
/// The cookie that holds a browser's session.
pub const SESSION_COOKIE: &str = "__Host-orch_session";
/// The cookie by which a browser its person signed in from is known.
pub const DEVICE_COOKIE: &str = "__Host-orch_device";
/// The media type of an error.
pub const PROBLEM_JSON: &str = "application/problem+json";

/// How many entries a list returns when it is not told.
pub const DEFAULT_LIMIT: u32 = 50;
/// The most entries a list returns at once.
pub const MAX_LIMIT: u32 = 200;
/// The most bytes of a JSON body.
pub const MAX_JSON_BYTES: usize = 1024 * 1024;
/// The most bytes of any single request body; more goes through an upload
/// session in pieces of [`UPLOAD_CHUNK_BYTES`].
pub const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

/// Optional false metadata stays absent in older complete-row representations.
pub(crate) const fn is_false(value: &bool) -> bool {
    !*value
}
/// The size of one piece of an upload.
pub const UPLOAD_CHUNK_BYTES: usize = 4 * 1024 * 1024;

#[cfg(test)]
mod listed_tests {
    use super::{HiddenItem, Id, Listed};
    use serde_json::json;

    #[test]
    fn a_private_name_has_no_readable_content_and_a_readable_row_is_flagged_as_such() {
        let hidden: Listed<serde_json::Value> = Listed::hidden(HiddenItem {
            id: Id::nil(),
            parent_id: None,
            kind: "provider".into(),
            name: "Private provider".into(),
            owner: None,
            content_hidden: true,
            private: true,
        });
        let wire = serde_json::to_value(&hidden).unwrap();
        assert_eq!(wire["content_hidden"], true);
        assert!(wire.get("facts").is_none());
        assert_eq!(
            serde_json::from_value::<Listed<serde_json::Value>>(wire).unwrap(),
            hidden
        );
        let visible =
            Listed::visible(json!({"id": Id::nil(), "name":"Readable provider", "status":"ready"}));
        let wire = serde_json::to_value(&visible).unwrap();
        assert_eq!(wire["content_hidden"], false);
        assert_eq!(wire["status"], "ready");
        assert_eq!(
            serde_json::from_value::<Listed<serde_json::Value>>(wire).unwrap(),
            visible
        );
    }
}

/// One page of a list that can grow. The order is stated by each list and
/// is stable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "{T}Page")]
pub struct Page<T> {
    /// The entries of this page.
    pub items: Vec<T>,
    /// What to send as `cursor` for the next page; `null` at the end.
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    /// The last page of a list, holding `items`.
    #[must_use]
    pub fn last(items: Vec<T>) -> Self {
        Self {
            items,
            next_cursor: None,
        }
    }
}

/// A named thing whose permission page may be managed while its content is private to the reader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HiddenItem {
    /// Its identifier.
    pub id: Id,
    /// Its structural parent or administrative home; null for the system's resource.
    pub parent_id: Option<Id>,
    /// Its kind.
    pub kind: String,
    /// Its name.
    pub name: String,
    /// Its first owner, when one is named.
    pub owner: Option<crate::users::UserRef>,
    /// Always true: none of the readable content shape is present.
    #[serde(deserialize_with = "true_value")]
    #[schemars(extend("const" = true))]
    pub content_hidden: bool,
    /// Always true: its sole destination is its permission page.
    #[serde(deserialize_with = "true_value")]
    #[schemars(extend("const" = true))]
    pub private: bool,
}

fn true_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    if bool::deserialize(deserializer)? {
        Ok(true)
    } else {
        Err(serde::de::Error::custom("must be true"))
    }
}

fn false_value<'de, D: Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    if !bool::deserialize(deserializer)? {
        Ok(false)
    } else {
        Err(serde::de::Error::custom("must be false"))
    }
}

/// A readable list row, with its existing content shape flattened beside the visibility flag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "{T}ListedContent")]
pub struct ListedContent<T> {
    /// The readable content.
    #[serde(flatten)]
    pub content: T,
    /// Always false: the readable shape is present.
    #[serde(deserialize_with = "false_value")]
    #[schemars(extend("const" = false))]
    pub content_hidden: bool,
}

/// A list row is either its readable content or the minimal name that opens its permission page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "{T}Listed")]
pub enum Listed<T> {
    /// Content hidden from an administering reader.
    Hidden(Box<HiddenItem>),
    /// Content the caller may read.
    Visible(ListedContent<T>),
}

impl<T> Listed<T> {
    /// Keeps the minimal permission-page row out of every readable row's inline storage.
    #[must_use]
    pub fn hidden(item: HiddenItem) -> Self {
        Self::Hidden(Box::new(item))
    }

    /// Wraps a readable row with its false visibility flag.
    #[must_use]
    pub fn visible(content: T) -> Self {
        Self::Visible(ListedContent {
            content,
            content_hidden: false,
        })
    }

    /// Takes the readable row, leaving a hidden row without any content to unwrap.
    #[must_use]
    pub fn into_visible(self) -> Option<T> {
        match self {
            Self::Visible(row) => Some(row.content),
            Self::Hidden(_) => None,
        }
    }

    /// The readable row, where it is one.
    #[must_use]
    pub const fn as_visible(&self) -> Option<&T> {
        match self {
            Self::Visible(row) => Some(&row.content),
            Self::Hidden(_) => None,
        }
    }

    /// The minimal row, where it is one.
    #[must_use]
    pub const fn as_hidden(&self) -> Option<&HiddenItem> {
        match self {
            Self::Hidden(row) => Some(row),
            Self::Visible(_) => None,
        }
    }
}

/// The query of a list that can grow.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    /// How many entries, at most [`MAX_LIMIT`]; [`DEFAULT_LIMIT`] when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

impl PageQuery {
    /// The number of entries to return.
    #[must_use]
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }
}

/// How much of something a person wants to be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionLevel {
    /// Everything.
    All,
    /// What asks for them.
    Important,
    /// Nothing.
    Off,
}

/// Reads a member that may be absent, `null` or a value, keeping the three
/// apart: absent is `None`, `null` is `Some(None)`. Use on a member of a
/// `PATCH` body with `#[serde(default, deserialize_with = "absent_or_null")]`.
pub fn absent_or_null<'de, T, D>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// One fault of a request that is not valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FieldFault {
    /// Where in the body, as `content[0].text`.
    pub path: String,
    /// What is wrong there.
    pub message: String,
}

macro_rules! error_codes {
    ($( $variant:ident = $status:literal, $code:literal, $title:literal; )*) => {
        /// Every error the API answers with: the stable machine value of
        /// an error, which a client localises by.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
        pub enum ErrorCode {
            $( #[doc = $title] #[serde(rename = $code)] $variant, )*
        }

        impl ErrorCode {
            /// Every code, in the order of the catalogue.
            pub const ALL: &'static [ErrorCode] = &[ $( ErrorCode::$variant, )* ];

            /// The code as it is written.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( ErrorCode::$variant => $code, )* }
            }

            /// The HTTP status it is answered with.
            #[must_use]
            pub const fn status(self) -> u16 {
                match self { $( ErrorCode::$variant => $status, )* }
            }

            /// A few English words, for logs and as a fallback.
            #[must_use]
            pub const fn title(self) -> &'static str {
                match self { $( ErrorCode::$variant => $title, )* }
            }
        }
    };
}

error_codes! {
    RequestInvalid = 400, "request.invalid", "The request is not valid";
    RequestValueRefused = 422, "request.value_refused", "A value is not one that can be kept";
    AdminRiskNotAcknowledged = 400, "admin.risk_not_acknowledged", "The risk was not acknowledged";
    EnvironmentNoneUsable = 400, "environment.none_usable", "No environment can be used";
    AuthRequired = 401, "auth.required", "Sign in first";
    AuthInvalidToken = 401, "auth.invalid_token", "The token is expired or unknown";
    AuthSuspended = 401, "auth.suspended", "This user is suspended";
    AuthInvalidCredentials = 401, "auth.invalid_credentials", "The name, the password or the code is wrong";
    AuthSecondStepRequired = 401, "auth.second_step_required", "A second step is needed";
    AuthSecondStepInvalid = 401, "auth.second_step_invalid", "The second step was wrong";
    OrgSuspended = 403, "org.suspended", "You are suspended in this org";
    AuthEmailUnverified = 403, "auth.email_unverified", "The address is not confirmed";
    AuthEmailCodeNotAllowed = 403, "auth.email_code_not_allowed", "A mailed code is not allowed as a second step";
    AuthLocalLoginDisabled = 403, "auth.local_login_disabled", "Signing in here is switched off";
    AuthSessionRequired = 403, "auth.session_required", "A browser session is needed";
    AuthCsrf = 403, "auth.csrf", "The request lacks its header";
    AuthPasswordCommon = 400, "auth.password_common", "This password is commonly used. Choose a different password or a longer phrase.";
    AuthSecondStepSetup = 403, "auth.second_step_setup", "A second step has to be set up first";
    AuthReauthenticationRequired = 403, "auth.reauthentication_required", "Prove who you are again for this";
    TokenLifetimeExceeded = 403, "token.lifetime_exceeded", "The token would live longer than is allowed here";
    TokenRefusedHere = 403, "token.refused_here", "This token is refused here";
    SharingExternalLifetimeExceeded = 403, "sharing.external_lifetime_exceeded", "Somebody from outside may be named here only until a nearer end";
    AuthzDenied = 403, "authz.denied", "You do not have access";
    RegistrationClosed = 403, "registration.closed", "Registration is closed";
    RegistrationInviteRequired = 403, "registration.invite_required", "An invitation is needed";
    SharingExternalForbidden = 403, "sharing.external_forbidden", "Somebody from outside may not be named here";
    ProviderSubscriptionNotAllowed = 403, "provider.subscription_not_allowed", "Subscriptions of this harness are not allowed here";
    ProviderNotAllowed = 403, "provider.not_allowed", "The provider is not allowed here";
    ProviderSubscriptionNotShared = 403, "provider.subscription_not_shared", "A subscription of this harness stays with its owner";
    WorkspaceRemoteWriteDisabled = 403, "workspace.remote_write_disabled", "Files are not written from here";
    PolicyRemoteWriteDisabled = 403, "policy.remote_write_disabled", "This machine keeps file writes off";
    TerminalNotAllowed = 403, "terminal.not_allowed", "A shell is not allowed here";
    BrowserNotAllowed = 403, "browser.not_allowed", "Browsers are not allowed here";
    VncNotAllowed = 403, "vnc.not_allowed", "Displays are not allowed here";
    VncTooMany = 409, "vnc.too_many", "The environment has as many displays as it allows";
    VncStartFailed = 502, "vnc.start_failed", "The display could not start";
    BrowserTooMany = 409, "browser.too_many", "The environment has as many browsers as it allows";
    BrowserProfileInUse = 409, "browser.profile_in_use", "A browser runs on this profile";
    BrowserStartFailed = 502, "browser.start_failed", "The browser could not start";
    PortsNotAllowed = 403, "ports.not_allowed", "This port use is not allowed";
    PortsTooMany = 409, "ports.too_many", "The wider port limit is reached";
    PortsNoRange = 409, "ports.no_range", "No acknowledged host port range is available";
    PortsPortInUse = 409, "ports.port_in_use", "The reviewed host port is in use";
    PortsProgramConflict = 409, "ports.program_conflict", "The port program needs a new review";
    PortsNotListening = 409, "ports.not_listening", "No owned service is listening";
    PortsDomainMissing = 409, "ports.domain_missing", "A browser ports domain is needed";
    PortsUnavailable = 503, "ports.unavailable", "The current port service is unavailable";
    ChatNotAllowed = 403, "chat.not_allowed", "This chat is not allowed";
    PolicyMcpImportDisabled = 403, "policy.mcp_import_disabled", "This machine lets no tool server be imported";
    PolicyMcpAddDisabled = 403, "policy.mcp_add_disabled", "This machine takes no tool server from here";
    DocsDisabled = 403, "docs.disabled", "Documents are switched off here";
    InvitationNotAllowed = 403, "invitation.not_allowed", "This form of invitation is not allowed";
    OrgCreationDisabled = 403, "org.creation_disabled", "Making a team org is restricted here";
    NotFound = 404, "not_found", "Not found";
    AccessLocked = 409, "access.locked", "This row is locked above";
    PolicyNotDelegated = 409, "policy.not_delegated", "The machine keeps these keys to itself";
    ConflictStaleRevision = 409, "conflict.stale_revision", "It was changed meanwhile";
    ConflictExists = 409, "conflict.exists", "It exists already";
    ConflictIdempotencyMismatch = 409, "conflict.idempotency_mismatch", "The key was used for another request";
    NodeArchived = 409, "node.archived", "It is archived";
    NodeRestoreParentFirst = 409, "node.restore_parent_first", "Bring back its parent first";
    NodeWrongParent = 409, "node.wrong_parent", "It cannot be put there";
    OrgTooDeep = 409, "org.too_deep", "Orgs would be nested too deep";
    ProjectTooDeep = 409, "project.too_deep", "Projects would be nested too deep";
    OrgClosed = 409, "org.closed", "This org takes no further member";
    AccessTooManyRequests = 409, "access.too_many_requests", "As many requests for access wait as are allowed";
    OrgFlat = 409, "org.flat", "This org holds no org inside it";
    OwnerLast = 409, "owner.last", "The last owner stays";
    MemberExists = 409, "member.exists", "They are a member already";
    MemberSameDomain = 409, "member.same_domain", "That is their domain already";
    MemberDomainRole = 409, "member.domain_role", "The role at a domain is changed, not removed";
    PolicyExecNotOffered = 409, "policy.exec_not_offered", "The machine does not offer that";
    ThreadNotIdle = 409, "thread.not_idle", "The thread is not idle";
    ThreadNotRunning = 409, "thread.not_running", "The thread is not running";
    ThreadUseSwitch = 409, "thread.use_switch", "A thread with history is changed by a switch";
    ThreadSide = 409, "thread.side", "A side thread does not take this";
    ThreadSideLimit = 409, "thread.side_limit", "No further side thread can be opened";
    GoalExists = 409, "goal.exists", "A goal is under way here already";
    GoalLimitReached = 409, "goal.limit_reached", "A limit of the goal is reached";
    InputNotPending = 409, "input.not_pending", "The input is not waiting any more";
    InputNotHeld = 409, "input.not_held", "The input is not held for its attachments";
    WorkspaceNotIndexed = 409, "workspace.not_indexed", "The workspace is too large to index";
    WorkspaceSourceConflict = 409, "workspace.source_conflict", "The source directory exists already";
    WorkspaceRestoreConflict = 409, "workspace.restore_conflict", "The restore plan no longer matches the directory";
    WorkspaceRestoreUnavailable = 409, "workspace.restore_unavailable", "The complete file state cannot be restored";
    GitConflict = 409, "git.conflict", "The repository changed since it was reviewed";
    ShareUnavailable = 409, "share.unavailable", "Nothing can be shared with this agent";
    CapabilityUnsupported = 409, "capability.unsupported", "The agent cannot do that";
    BindingIncompatible = 409, "binding.incompatible", "These do not work together";
    AutoDisabled = 409, "auto.disabled", "That is not done automatically here";
    PlanBlocked = 409, "plan.blocked", "The plan cannot be carried out";
    NativeActionUnavailable = 409, "native_action.unavailable", "The selected command is unavailable in this session";
    NativeActionStale = 409, "native_action.stale", "The session's command list changed";
    ProviderHandedOver = 409, "provider.handed_over", "The credential is with another environment";
    CredentialReturnFailed = 409, "credential.return_failed", "The credential could not be brought home";
    ModelNotAllowed = 409, "model.not_allowed", "The model is not allowed";
    ApprovalAlreadyDecided = 409, "approval.already_decided", "It was decided already";
    PreflightExpired = 409, "preflight.expired", "The plan is out of date";
    ResourceInUse = 409, "resource.in_use", "It is still in use";
    AdminLastAdmin = 409, "admin.last_admin", "The last admin stays";
    UploadOffset = 409, "upload.offset", "The piece does not continue the upload";
    RouteNotReady = 409, "route.not_ready", "The route cannot reach its person yet";
    ExportTooLarge = 409, "export.too_large", "The archive would be larger than the place allows";
    ExportTooMany = 409, "export.too_many", "The archive would hold more things than the place allows";
    ExportNothing = 409, "export.nothing", "There is nothing in it that you may take away";
    ImportTooLarge = 409, "import.too_large", "The import would be larger than the place allows";
    ImportTooMany = 409, "import.too_many", "The import would hold more things than the place allows";
    ImportNothing = 409, "import.nothing", "There is nothing in it that you may bring here";
    ImportInvalid = 422, "import.invalid", "The source is not a package or files that can be imported";
    ImportUndone = 409, "import.undone", "The import was taken back already, or is being";
    Gone = 410, "gone", "It expired or was used";
    DocsTooLarge = 413, "docs.too_large", "The document would be too large";
    ShareTooLarge = 413, "share.too_large", "The copy would be too large";
    RequestTooLarge = 413, "request.too_large", "The request is too large";
    SettingsInstructionsTooLarge = 413, "settings.instructions_too_large", "The standing instructions would be longer than they may be";
    WorkspaceSourceFailed = 422, "workspace.source_failed", "The source could not be prepared";
    GitFailed = 422, "git.failed", "Git refused the requested operation";
    AuthLocked = 423, "auth.locked", "Too many wrong tries";
    RateLimited = 429, "rate_limited", "Too many requests";
    Internal = 500, "internal", "Something went wrong";
    SourceOffline = 503, "source.offline", "The machine is offline";
    AgentOffline = 503, "agent.offline", "The agent is offline";
    ProviderUnavailable = 503, "provider.unavailable", "The provider is not ready";
    HarnessSessionsNoRuntime = 409, "harness.sessions_no_runtime", "No agent runtime holds these sessions";
    HarnessSessionsUnavailable = 503, "harness.sessions_unavailable", "The agent's sessions could not be read";
    HarnessChannelsUnavailable = 503, "harness.channels_unavailable", "The release channels could not be read";
    ProviderResetStale = 409, "provider.reset_stale", "The reset confirmation is stale";
    ProviderResetUnavailable = 503, "provider.reset_unavailable", "The provider cannot use a reset";
    EnvironmentUnavailable = 503, "environment.unavailable", "The environment is not ready";
    NetworkRestrictedUnavailable = 409, "network.restricted_unavailable", "The requested network restriction is unavailable";
    NetworkHostDenied = 403, "network.host_denied", "The network policy refused this host";
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error, as it is answered: `application/problem+json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{code}: {detail}")]
pub struct Problem {
    /// `orch:error:` followed by the code.
    #[serde(rename = "type")]
    pub kind: String,
    /// The stable machine value.
    pub code: ErrorCode,
    /// A few English words.
    pub title: String,
    /// The HTTP status.
    pub status: u16,
    /// One English sentence about this occurrence.
    pub detail: String,
    /// The request this answers, as in the `X-Request-Id` header.
    pub request_id: String,
    /// What the code adds, member by member.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Json>,
}

impl Problem {
    /// An error of `code`, saying `detail`. The request id is filled in
    /// where the answer is written.
    #[must_use]
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            kind: format!("orch:error:{}", code.as_str()),
            code,
            title: code.title().to_owned(),
            status: code.status(),
            detail: detail.into(),
            request_id: String::new(),
            extra: serde_json::Map::new(),
        }
    }

    /// Adds a member that the code defines.
    #[must_use]
    pub fn with(mut self, member: &str, value: impl Serialize) -> Self {
        let value = serde_json::to_value(value).unwrap_or(Json::Null);
        self.extra.insert(member.to_owned(), value);
        self
    }

    /// A request that is not valid, with each fault.
    #[must_use]
    pub fn invalid(faults: Vec<FieldFault>) -> Self {
        let detail = faults.first().map_or_else(
            || "the request is not valid".to_owned(),
            |fault| format!("{}: {}", fault.path, fault.message),
        );
        Self::new(ErrorCode::RequestInvalid, detail).with("errors", faults)
    }

    /// A request with one fault at `path`.
    #[must_use]
    pub fn invalid_at(path: &str, message: &str) -> Self {
        Self::invalid(vec![FieldFault {
            path: path.to_owned(),
            message: message.to_owned(),
        }])
    }

    /// Nothing is there, or the caller may not know that it is.
    #[must_use]
    pub fn not_found() -> Self {
        Self::new(
            ErrorCode::NotFound,
            "there is no such thing, or it was deleted",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_problem_is_written_flat_with_what_its_code_adds() {
        let problem = Problem::new(
            ErrorCode::AuthzDenied,
            "thread.interact is required on this thread",
        )
        .with("required", "thread.interact")
        .with("requestable", true);
        let written = serde_json::to_value(&problem).unwrap();
        assert_eq!(written["type"], "orch:error:authz.denied");
        assert_eq!(written["code"], "authz.denied");
        assert_eq!(written["status"], 403);
        assert_eq!(written["required"], "thread.interact");
        assert_eq!(serde_json::from_value::<Problem>(written).unwrap(), problem);
    }

    #[test]
    fn codes_are_unique_and_their_statuses_are_errors() {
        let mut seen = std::collections::HashSet::new();
        for code in ErrorCode::ALL {
            assert!(seen.insert(code.as_str()), "{code} twice");
            assert!((400..600).contains(&code.status()), "{code}");
            assert_eq!(serde_json::to_value(code).unwrap(), json!(code.as_str()));
        }
    }

    #[test]
    fn a_list_is_asked_for_within_its_bounds() {
        assert_eq!(PageQuery::default().limit(), 50);
        assert_eq!(
            PageQuery {
                limit: Some(0),
                cursor: None
            }
            .limit(),
            1
        );
        assert_eq!(
            PageQuery {
                limit: Some(9_999),
                cursor: None
            }
            .limit(),
            200
        );
    }

    #[test]
    fn absent_null_and_a_value_are_three_things() {
        #[derive(Deserialize)]
        struct Patch {
            #[serde(default, deserialize_with = "absent_or_null")]
            note: Option<Option<String>>,
        }
        assert_eq!(
            serde_json::from_value::<Patch>(json!({})).unwrap().note,
            None
        );
        assert_eq!(
            serde_json::from_value::<Patch>(json!({ "note": null }))
                .unwrap()
                .note,
            Some(None)
        );
        assert_eq!(
            serde_json::from_value::<Patch>(json!({ "note": "x" }))
                .unwrap()
                .note,
            Some(Some("x".into()))
        );
    }
}
