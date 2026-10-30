//! Sign-in on the provider (a code, an existing sign-in file given as text or by its path inside the runtime, a workspace token), the workspace restriction, usage facts and model suggestions.
//!
//! A subscription of Codex is signed in by one of three ways, each the vendor's own:
//!
//! * `device_code` (interactive, the one a dialog opens on): the harness shows an address and a one-time
//!   code, and the person enters the code there. The harness says when it is done.
//! * `auth_file` (import): the contents of an `auth.json` that `codex login` wrote with file storage, given
//!   as text or as the path of a file inside the provider's runtime. The adapter checks that it is one,
//!   writes it into the provider's home (readable by the user of the process alone), and starts the harness again, since a
//!   running harness does not see a sign-in written behind its back. From then on the harness renews that
//!   copy here; the vendor's guide says to use one copy in one place, because whichever of two places
//!   renews first leaves the other unable to.
//! * `access_token` (import): a workspace token given to the harness's own command on standard input,
//!   or a ChatGPT access token stored in the login file without a refresh token. Its own claims give its
//!   expiry and issuance time; the original sign-in keeps working elsewhere.
//!
//! All three end in the same state: a home that holds the sign-in, and a harness that reports itself signed in.
//!
//! What the harness reports of the provider (who is signed in, the plan, how full its windows are, what the
//! plan's credits have left, a limit that is reached) becomes [`ProviderFacts`], and what it lists as models
//! becomes suggestions: a proposal and no more. The speeds a model has beside the ordinary one and its access
//! programs are listed as *offered*; a manager of the provider enables them. The harness's list says nothing of a
//! model's window; its catalogue, which the harness renders on request, does ([`read_windows`]).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::discovery::ModelSuggestion;
use orch_agent_contract::facts::{
    AvailableResets, CredentialFacts, CredentialYield, LimitReached, PlanCredits, ProviderFacts,
    ResetCredit, ResetRequest, ResetResult, UsageWindow,
};
use orch_agent_contract::login::{
    LoginKind, LoginMethodDef, LoginShow, LoginState, LoginStateKind, LoginYields, ShowKind,
};
use orch_agent_contract::parameters::{ModelParameters, Offered};
use orch_types::{Clock, Field, FieldType, Json, Time};
use serde_json::json;
use tokio::io::AsyncReadExt as _;

use crate::parameters::{offered_for, window_of};
use crate::process::{AUTH_FILE, LaunchRequest, Launcher, write_private};
use crate::protocol::{Rpc, RpcError};

/// How long a one-time code of the device flow can be used: fifteen minutes, which is what the vendor's
/// sign-in page allows.
pub const DEVICE_CODE_LIFETIME: Duration = Duration::from_secs(15 * 60);

/// The most an `auth.json` can be: 64 KiB, which holds three tokens many times over.
pub const AUTH_FILE_MAX_BYTES: u64 = 65_536;

/// The file of the home in which the adapter notes how the harness was signed in, for the facts it reports.
pub const LOGIN_NOTE_FILE: &str = "orch-login.json";

/// How long a request for the account may take: the harness asks the vendor's servers to find the
/// workspace of a ChatGPT sign-in, and without a network that waits for its own timeout.
pub const ACCOUNT_TIMEOUT: Duration = Duration::from_secs(12);

/// The command of the harness that renders its raw catalogue of models as JSON, which carries the window each
/// model starts with and the largest an override may set; the list the server answers carries neither.
pub const CATALOGUE_COMMAND: [&str; 2] = ["debug", "models"];

/// How long the catalogue may take to be rendered.
pub const CATALOGUE_TIMEOUT: Duration = Duration::from_secs(30);

/// The most of the catalogue that is read, in bytes: it is about 650 000 today, and a harness that writes
/// without end is cut off at a size no catalogue reaches.
pub const CATALOGUE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// The ids of the sign-in methods.
pub mod method {
    /// The code entered on the vendor's page.
    pub const DEVICE_CODE: &str = "device_code";
    /// The contents of an `auth.json`.
    pub const AUTH_FILE: &str = "auth_file";
    /// A workspace or ChatGPT access token.
    pub const ACCESS_TOKEN: &str = "access_token";
}

fn text_field(name: &str, label: &str, kind: FieldType, help: &str) -> Field {
    let mut field = Field::new(name, label, kind);
    field.required = false;
    field.help = Some(help.to_owned());
    field
}

/// Every way a provider of this harness can be signed in.
#[must_use]
pub fn login_methods() -> Vec<LoginMethodDef> {
    let mut auth_json = text_field(
        "auth_json",
        "Contents of auth.json",
        FieldType::SecretText,
        "Paste the file `codex login` wrote when it kept the sign-in in a file. Give this or the path below, not both.",
    );
    auth_json.max_bytes = Some(AUTH_FILE_MAX_BYTES);
    let auth_path = text_field(
        "auth_path",
        "Path of an auth.json in the runtime",
        FieldType::Text,
        "For a runtime whose image or template brings the file: the path inside it. Give this or the contents above, not both.",
    );
    let mut token = Field::new("access_token", "Access token", FieldType::Secret);
    token.required = true;
    token.help = Some(
        "Use a token issued by your workspace's administrators, or the access_token from the tokens object in the auth.json of your existing ChatGPT sign-in. Bring only the access token. Orch never renews it, and the sign-in it came from continues working there.".to_owned(),
    );
    let mut expires = Field::new("expires_at", "Valid until", FieldType::Date);
    expires.required = false;
    expires.help =
        Some("The day the token lapses, if you know it; a notice is sent before.".to_owned());
    vec![
        LoginMethodDef {
            id: method::DEVICE_CODE.to_owned(),
            title: "Sign in with a code".to_owned(),
            summary: "Orch shows a short code. You enter it on the vendor's sign-in page.".to_owned(),
            kind: LoginKind::Interactive,
            fields: Vec::new(),
            yields: LoginYields::Login,
            lifetime_secs: None,
            requires: Some(
                "Device-code sign-in has to be switched on in your ChatGPT security settings, or by your workspace's administrators."
                    .to_owned(),
            ),
            recommended: true,
        },
        LoginMethodDef {
            id: method::AUTH_FILE.to_owned(),
            title: "Bring an auth.json".to_owned(),
            summary: "Copies a sign-in you already have. From then on this copy is renewed here. Use one copy in one place: where the same file stays in use elsewhere, whichever of the two renews first leaves the other unable to renew. A sign-in that is kept fresh elsewhere is copied in again by the same method.".to_owned(),
            kind: LoginKind::Import,
            fields: vec![auth_json, auth_path],
            yields: LoginYields::Login,
            lifetime_secs: None,
            requires: None,
            recommended: false,
        },
        LoginMethodDef {
            id: method::ACCESS_TOKEN.to_owned(),
            title: "Use an access token".to_owned(),
            summary: "Accepts a workspace token or a ChatGPT sign-in's access token. Orch never renews it: bring a fresh token before it lapses. The sign-in it came from continues working where it is.".to_owned(),
            kind: LoginKind::Import,
            fields: vec![token, expires],
            yields: LoginYields::Token,
            lifetime_secs: None,
            requires: Some("For a workspace token, your workspace's administrators must allow it. For ChatGPT, use only the access token of your existing sign-in, never its refresh token.".to_owned()),
            recommended: false,
        },
    ]
}

/// The positive initialize wait configured by this provider, or the startup default.
pub fn initialize_timeout(config: &Json) -> Result<Duration, AdapterError> {
    let Some(value) = config.get("initialize_timeout_secs") else {
        return Ok(crate::protocol::INITIALIZE_TIMEOUT);
    };
    let seconds = value
        .as_u64()
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| {
            crate::invalid("initialize_timeout_secs must be a positive whole number of seconds")
        })?;
    let timeout = Duration::from_secs(seconds);
    if tokio::time::Instant::now().checked_add(timeout).is_none() {
        return Err(crate::invalid(
            "initialize_timeout_secs exceeds the time the monotonic clock can represent",
        ));
    }
    Ok(timeout)
}

/// What a provider may configure about its account workspace and native startup.
#[must_use]
pub fn options_schema() -> Json {
    json!({
        "type": "object",
        "properties": {
            "workspace_id": {
                "type": "string",
                "title": "Workspace",
                "description": "The ChatGPT workspace a sign-in of this provider must belong to. With a sign-in from anywhere else the harness signs itself out."
            },
            "initialize_timeout_secs": {
                "type": "integer",
                "minimum": 1,
                "default": crate::protocol::INITIALIZE_TIMEOUT.as_secs(),
                "x-unit": "seconds"
            }
        },
        "additionalProperties": false
    })
}

// ---------------------------------------------------------------------------------------------------------
// The sign-in that runs

#[derive(Debug, Default)]
struct Tracked {
    login_id: Option<String>,
    method: Option<String>,
    identity: Option<String>,
    plan: Option<String>,
    expires_at: Option<Time>,
    issued_at: Option<Time>,
    signed_in: bool,
    facts: Option<ProviderFacts>,
}

/// What the adapter knows of the sign-in that runs and of who is signed in.
#[derive(Debug, Default)]
pub struct LoginTracker {
    inner: Mutex<Tracked>,
}

impl LoginTracker {
    fn with<R>(&self, f: impl FnOnce(&mut Tracked) -> R) -> R {
        f(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// A sign-in by `method` that the harness knows as `login_id` was started.
    pub fn started(&self, login_id: &str, method: &str) {
        self.with(|tracked| {
            tracked.login_id = Some(login_id.to_owned());
            tracked.method = Some(method.to_owned());
        });
    }

    /// The sign-in that was running is over: what the harness says of it, as the state it leaves.
    pub fn completed(&self, params: &Json) -> Option<LoginState> {
        self.with(|tracked| {
            let id = params.get("loginId").and_then(Json::as_str);
            if tracked.login_id.is_some() && id.is_some() && tracked.login_id.as_deref() != id {
                return None;
            }
            let method = tracked
                .method
                .clone()
                .unwrap_or_else(|| method::DEVICE_CODE.to_owned());
            tracked.login_id = None;
            let success = params
                .get("success")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            tracked.signed_in = success;
            Some(if success {
                LoginState::completed(&method)
            } else {
                let reason = params
                    .get("error")
                    .and_then(Json::as_str)
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or("the sign-in did not complete");
                LoginState::failed(&method, reason)
            })
        })
    }

    /// The harness's id for the sign-in that runs.
    #[must_use]
    pub fn login_id(&self) -> Option<String> {
        self.with(|tracked| tracked.login_id.clone())
    }

    /// Forgets the sign-in that ran.
    pub fn clear(&self) {
        self.with(|tracked| tracked.login_id = None);
    }

    /// Notes who is signed in, as the harness or the file says.
    pub fn signed_in_as(&self, identity: Option<String>, plan: Option<String>) {
        self.with(|tracked| {
            tracked.signed_in = true;
            tracked.identity = identity;
            tracked.plan = plan;
        });
    }

    /// Notes how the sign-in was made, as read back from the home.
    pub fn noted(&self, method: Option<String>, expires_at: Option<Time>) {
        self.with(|tracked| {
            tracked.method = method;
            tracked.expires_at = expires_at;
        });
    }

    /// Notes the issuance time carried by an imported token.
    pub fn issued(&self, at: Option<Time>) {
        self.with(|tracked| tracked.issued_at = at);
    }

    fn cache(&self, facts: ProviderFacts) -> ProviderFacts {
        self.with(|tracked| tracked.facts = Some(facts.clone()));
        facts
    }

    fn cached(&self) -> Option<ProviderFacts> {
        self.with(|tracked| tracked.facts.clone())
    }

    /// Notes that nobody is signed in.
    pub fn signed_out(&self) {
        self.with(|tracked| *tracked = Tracked::default());
    }

    fn snapshot(
        &self,
    ) -> (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<Time>,
        bool,
    ) {
        self.with(|tracked| {
            (
                tracked.identity.clone(),
                tracked.plan.clone(),
                tracked.method.clone(),
                tracked.expires_at,
                tracked.signed_in,
            )
        })
    }
}

/// The note of how a home was signed in.
pub fn write_login_note(
    home: &Path,
    method: &str,
    expires_at: Option<Time>,
) -> std::io::Result<()> {
    let note = json!({ "method": method, "expires_at": expires_at });
    write_private(&home.join(LOGIN_NOTE_FILE), note.to_string().as_bytes())
}

/// The method and the end of the sign-in a home was last signed in by, if the note is there.
#[must_use]
pub fn read_login_note(home: &Path) -> (Option<String>, Option<Time>) {
    let Ok(text) = std::fs::read_to_string(home.join(LOGIN_NOTE_FILE)) else {
        return (None, None);
    };
    let Ok(note) = serde_json::from_str::<Json>(&text) else {
        return (None, None);
    };
    (
        note.get("method").and_then(Json::as_str).map(str::to_owned),
        note.get("expires_at")
            .and_then(Json::as_str)
            .and_then(|at| at.parse().ok()),
    )
}

/// The state of a device-code sign-in the harness started: the address to open and the code to enter.
#[must_use]
pub fn device_code_state(started: &Json, now: Time) -> LoginState {
    let expires = chrono::Duration::from_std(DEVICE_CODE_LIFETIME).unwrap_or_default();
    LoginState {
        state: LoginStateKind::AwaitingUser,
        method: Some(method::DEVICE_CODE.to_owned()),
        shows: vec![
            LoginShow {
                kind: ShowKind::Uri,
                label: "Open".to_owned(),
                value: started
                    .get("verificationUrl")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
            LoginShow {
                kind: ShowKind::Code,
                label: "Code".to_owned(),
                value: started
                    .get("userCode")
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            },
        ],
        asks: None,
        expires_at: Some(Time::new(now.get() + expires)),
        error: None,
    }
}

// ---------------------------------------------------------------------------------------------------------
// An auth.json

/// What an `auth.json` says of who is signed in.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AuthSummary {
    /// The email of the account, from the claims of its identity token.
    pub email: Option<String>,
    /// The plan, as the claims name it.
    pub plan: Option<String>,
    /// The workspace the sign-in belongs to.
    pub account_id: Option<String>,
}

fn claims_of(token: &str) -> Option<Json> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// The two kinds of access token this harness can be given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessTokenKind {
    /// The ChatGPT claims can be stored as the harness's login without a refresh token.
    Chatgpt,
    /// The harness's workspace-token command validates the workspace's own claims.
    Workspace,
}

/// The kind and lifetime read from an access token, without retaining its secret text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessTokenInfo {
    /// Which import path to use.
    pub kind: AccessTokenKind,
    /// When the token was issued, if it says.
    pub issued_at: Option<Time>,
    /// When it lapses.
    pub expires_at: Option<Time>,
}

/// The pinned harness identifies its opaque administrator-issued access tokens by this prefix.
const WORKSPACE_ACCESS_TOKEN_PREFIX: &str = "at-";

fn token_refused(reason: &str) -> AdapterError {
    AdapterError::Other {
        code: "login_refused".to_owned(),
        message: reason.to_owned(),
        retryable: false,
    }
}

fn epoch(value: Option<&Json>) -> Option<Time> {
    value?
        .as_i64()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(Time::new)
}

/// Reads a token's kind and expiry; the harness, then the vendor, still validate the credential itself.
pub fn check_access_token(token: &str, now: Time) -> Result<AccessTokenInfo, AdapterError> {
    let invalid =
        || token_refused("This is neither a workspace access token nor a ChatGPT access token.");
    if token.starts_with(WORKSPACE_ACCESS_TOKEN_PREFIX)
        && token.chars().all(|c| !c.is_whitespace() && !c.is_control())
    {
        return Ok(AccessTokenInfo {
            kind: AccessTokenKind::Workspace,
            issued_at: None,
            expires_at: None,
        });
    }
    if token.split('.').count() != 3 {
        return Err(invalid());
    }
    let claims = claims_of(token)
        .filter(Json::is_object)
        .ok_or_else(invalid)?;
    let expires_at = epoch(claims.get("exp")).ok_or_else(invalid)?;
    let issued_at = epoch(claims.get("iat"));
    if issued_at.is_some_and(|at| at > now || at >= expires_at) {
        return Err(invalid());
    }
    let auth = claims.get("https://api.openai.com/auth");
    let named = |name| {
        auth.and_then(|auth| auth.get(name))
            .and_then(Json::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    let kind = if named("chatgpt_account_id") && named("chatgpt_plan_type") {
        AccessTokenKind::Chatgpt
    } else {
        AccessTokenKind::Workspace
    };
    if expires_at <= now {
        return Err(token_refused(match kind {
            AccessTokenKind::Chatgpt => {
                "The ChatGPT access token has already lapsed. Bring a fresh access token."
            }
            AccessTokenKind::Workspace => {
                "The workspace access token has already lapsed. Bring a fresh access token."
            }
        }));
    }
    Ok(AccessTokenInfo {
        kind,
        issued_at,
        expires_at: Some(expires_at),
    })
}

/// Stores a ChatGPT access token in the harness's login format, with no credential for renewal.
pub fn write_chatgpt_access_token(home: &Path, token: &str, now: Time) -> Result<(), AdapterError> {
    let claims = claims_of(token)
        .ok_or_else(|| token_refused("The access token has no readable claims."))?;
    let account = claims
        .get("https://api.openai.com/auth")
        .and_then(|auth| auth.get("chatgpt_account_id"));
    let file = json!({
        "auth_mode": "chatgpt", "OPENAI_API_KEY": null,
        "tokens": { "access_token": token, "id_token": token, "refresh_token": "", "account_id": account },
        "last_refresh": now
    });
    write_private(&home.join(AUTH_FILE), file.to_string().as_bytes())
        .map_err(|_| token_refused("The provider could not keep the sign-in."))
}

/// Keeps how an access token was imported and the lifetime its own claims gave.
pub fn write_access_note(home: &Path, info: &AccessTokenInfo) -> std::io::Result<()> {
    write_private(&home.join(LOGIN_NOTE_FILE), json!({
        "method": method::ACCESS_TOKEN, "expires_at": info.expires_at, "issued_at": info.issued_at
    }).to_string().as_bytes())
}

/// The issuance time kept with an imported access token.
#[must_use]
pub fn read_issued_at(home: &Path) -> Option<Time> {
    let note: Json =
        serde_json::from_str(&std::fs::read_to_string(home.join(LOGIN_NOTE_FILE)).ok()?).ok()?;
    note.get("issued_at").and_then(Json::as_str)?.parse().ok()
}

/// Checks that `text` is an `auth.json` of the shape the harness writes (a sign-in with tokens, or an API
/// key) and reads who it is for. The refusal never quotes the text.
pub fn check_auth_file(text: &str) -> Result<AuthSummary, AdapterError> {
    let refuse = |why: &str| AdapterError::Other {
        code: "invalid_login_values".to_owned(),
        message: format!("this is not an auth.json: {why}"),
        retryable: false,
    };
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > AUTH_FILE_MAX_BYTES {
        return Err(refuse("it is larger than any the harness writes"));
    }
    let auth: Json = serde_json::from_str(text).map_err(|_| refuse("it is not JSON"))?;
    if !auth.is_object() {
        return Err(refuse("it is not an object"));
    }
    let tokens = auth.get("tokens").filter(|tokens| tokens.is_object());
    let has_key = auth
        .get("OPENAI_API_KEY")
        .and_then(Json::as_str)
        .is_some_and(|key| !key.is_empty());
    if tokens.is_none() && !has_key {
        return Err(refuse("it holds neither tokens nor a key"));
    }
    if let Some(tokens) = tokens {
        let present = |name: &str| {
            tokens
                .get(name)
                .and_then(Json::as_str)
                .is_some_and(|value| !value.is_empty())
        };
        if !present("access_token") || !present("refresh_token") {
            return Err(refuse(
                "its tokens lack the access token or the refresh token",
            ));
        }
    }
    let claims = tokens
        .and_then(|tokens| tokens.get("id_token"))
        .and_then(Json::as_str)
        .and_then(claims_of);
    let auth_claims = claims
        .as_ref()
        .and_then(|claims| claims.get("https://api.openai.com/auth"));
    Ok(AuthSummary {
        email: claims
            .as_ref()
            .and_then(|claims| claims.get("email"))
            .and_then(Json::as_str)
            .map(str::to_owned),
        plan: auth_claims
            .and_then(|claims| claims.get("chatgpt_plan_type"))
            .and_then(Json::as_str)
            .map(str::to_owned),
        account_id: auth_claims
            .and_then(|claims| claims.get("chatgpt_account_id"))
            .and_then(Json::as_str)
            .map(str::to_owned),
    })
}

/// Reads the `auth.json` of a home, if there is one.
#[must_use]
pub fn read_auth_summary(home: &Path) -> Option<AuthSummary> {
    let text = std::fs::read_to_string(home.join(AUTH_FILE)).ok()?;
    if let Ok(summary) = check_auth_file(&text) {
        return Some(summary);
    }
    let file: Json = serde_json::from_str(&text).ok()?;
    if file
        .get("tokens")?
        .get("refresh_token")
        .and_then(Json::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        return None;
    }
    let claims = file
        .get("tokens")?
        .get("access_token")?
        .as_str()
        .and_then(claims_of)?;
    let auth = claims.get("https://api.openai.com/auth")?;
    Some(AuthSummary {
        email: claims
            .get("email")
            .and_then(Json::as_str)
            .map(str::to_owned),
        plan: auth
            .get("chatgpt_plan_type")
            .and_then(Json::as_str)
            .map(str::to_owned),
        account_id: auth
            .get("chatgpt_account_id")
            .and_then(Json::as_str)
            .map(str::to_owned),
    })
}

// ---------------------------------------------------------------------------------------------------------
// Facts

/// The name of a window of the plan: `5h`, `weekly`, or its length in hours, days or minutes.
#[must_use]
pub fn window_label(minutes: Option<i64>) -> String {
    const WEEK: i64 = 7 * 24 * 60;
    const DAY: i64 = 24 * 60;
    match minutes {
        None => "window".to_owned(),
        Some(WEEK) => "weekly".to_owned(),
        Some(minutes) if minutes % DAY == 0 && minutes >= DAY => format!("{}d", minutes / DAY),
        Some(minutes) if minutes % 60 == 0 && minutes >= 60 => format!("{}h", minutes / 60),
        Some(minutes) => format!("{minutes}m"),
    }
}

/// The vendor slides an unused window's reset with its own clock; one minute covers second precision and a read in flight.
const UNSTARTED_RESET_TOLERANCE_SECONDS: i64 = 60;

fn window(id: &str, snapshot: &Json, now: Time) -> Option<UsageWindow> {
    let window = snapshot.get(id).filter(|window| window.is_object())?;
    let minutes = window.get("windowDurationMins").and_then(Json::as_i64);
    let used_percent = window.get("usedPercent").and_then(Json::as_f64)?;
    let resets_at = epoch(window.get("resetsAt"));
    let unstarted = used_percent == 0.0
        && minutes
            .filter(|minutes| *minutes > 0)
            .zip(resets_at)
            .is_some_and(|(minutes, reset)| {
                let expected = chrono::Duration::try_minutes(minutes)
                    .and_then(|length| now.get().checked_add_signed(length));
                expected.is_some_and(|expected| {
                    (reset.get() - expected).num_seconds().abs()
                        <= UNSTARTED_RESET_TOLERANCE_SECONDS
                })
            });
    Some(UsageWindow {
        id: id.to_owned(),
        label: window_label(minutes),
        duration_minutes: minutes,
        used_percent,
        resets_at: resets_at.filter(|_| !unstarted),
        model_family: None,
        is_active: None,
        reset_not_started: resets_at.map(|_| unstarted),
        warning_percent: None,
        severity: None,
    })
}

/// The facts one snapshot of the plan's limits comes to, with who is signed in as the tracker knows.
#[must_use]
pub fn facts_from_snapshot(snapshot: &Json, tracker: &LoginTracker, now: Time) -> ProviderFacts {
    let (identity, tracked_plan, method, expires_at, signed_in) = tracker.snapshot();
    let windows: Vec<UsageWindow> = ["primary", "secondary"]
        .into_iter()
        .filter_map(|id| window(id, snapshot, now))
        .collect();
    let plan_credits = snapshot
        .get("credits")
        .filter(|credits| credits.get("unlimited").and_then(Json::as_bool).is_some())
        .map(|credits| PlanCredits {
            balance: credits
                .get("balance")
                .and_then(Json::as_str)
                .map(str::to_owned),
            unit: Some("credits".to_owned()),
            unlimited: credits
                .get("unlimited")
                .and_then(Json::as_bool)
                .unwrap_or(false),
        });
    let limit_reached = snapshot
        .get("rateLimitReachedType")
        .and_then(Json::as_str)
        .map(|kind| LimitReached {
            kind: kind.to_owned(),
            resets_at: windows.iter().filter_map(|window| window.resets_at).min(),
        });
    let token = method.as_deref() == Some(method::ACCESS_TOKEN);
    ProviderFacts {
        plan: snapshot
            .get("planType")
            .and_then(Json::as_str)
            .map(str::to_owned)
            .or(tracked_plan),
        windows,
        plan_credits,
        limit_reached,
        identity,
        credential: signed_in.then_some(CredentialFacts {
            method,
            yields: if token {
                CredentialYield::Token
            } else {
                CredentialYield::Login
            },
            expires_at,
            issued_at: tracker.with(|tracked| tracked.issued_at),
        }),
        ..ProviderFacts::default()
    }
}

/// The facts a notification about the account or its limits comes to, if it says anything.
#[must_use]
pub fn facts_from_update(
    method: &str,
    params: &Json,
    tracker: &LoginTracker,
    now: Time,
) -> Option<ProviderFacts> {
    match method {
        "account/rateLimits/updated" => {
            let snapshot = params.get("rateLimits")?;
            let says_something = [
                "planType",
                "primary",
                "secondary",
                "credits",
                "rateLimitReachedType",
            ]
            .into_iter()
            .any(|member| snapshot.get(member).is_some_and(|value| !value.is_null()));
            says_something.then(|| {
                let mut facts = facts_from_snapshot(snapshot, tracker, now);
                if let Some(previous) = tracker.cached() {
                    facts.available_resets = previous.available_resets;
                }
                if let Some(previous) = tracker.cached() {
                    facts.field_times = previous.field_times;
                }
                if snapshot.get("credits").is_some() {
                    facts.field_times.insert("plan_credits".to_owned(), now);
                }
                facts.credential_verified = None;
                for field in ["windows", "plan", "limit_reached"] {
                    facts.field_times.insert(field.to_owned(), now);
                }
                for window in &facts.windows {
                    facts
                        .field_times
                        .insert(format!("windows.{}", window.id), now);
                }
                let mut outgoing = tracker.cache(facts);
                // This notification carries windows, not a new observation of the reset credits.
                outgoing.available_resets = None;
                outgoing
            })
        }
        "account/updated" => {
            let auth_mode = params.get("authMode")?;
            let plan = params
                .get("planType")
                .and_then(Json::as_str)
                .map(str::to_owned);
            if auth_mode.is_null() {
                tracker.signed_out();
                return Some(ProviderFacts::default());
            }
            let (identity, _, _, _, _) = tracker.snapshot();
            tracker.signed_in_as(identity, plan);
            let mut facts = tracker.cached().unwrap_or_default();
            let (identity, plan, _, _, _) = tracker.snapshot();
            facts.identity = identity;
            facts.plan = plan;
            facts.credential_verified = None;
            for field in ["identity", "plan"] {
                facts.field_times.insert(field.to_owned(), now);
            }
            Some(tracker.cache(facts))
        }
        _ => None,
    }
}

fn remote_text(error: &RpcError) -> String {
    match error {
        RpcError::Remote { message, .. } => message.clone(),
        other => other.to_string(),
    }
}

/// Asks the harness whether it is signed in, which it answers without the network. `Some(mode)` is signed
/// in (`chatgpt`, `apikey`, …).
pub async fn auth_mode(rpc: &Rpc) -> Result<Option<String>, AdapterError> {
    let status = rpc
        .call(
            "getAuthStatus",
            json!({ "includeToken": false, "refreshToken": false }),
        )
        .await
        .map_err(|error| AdapterError::HarnessError(remote_text(&error)))?;
    match status.get("authMethod") {
        Some(Json::Null) => Ok(None),
        Some(Json::String(mode)) if !mode.is_empty() => Ok(Some(mode.clone())),
        _ => Err(not_told()),
    }
}

/// Requests native renewal explicitly, then makes the ordinary vendor observation again. Imported access
/// tokens and API keys are never renewed, even if the controller's earlier facts were stale.
pub async fn renew(
    rpc: &Rpc,
    tracker: &LoginTracker,
    home: Option<&Path>,
    clock: &dyn Clock,
) -> Result<ProviderFacts, AdapterError> {
    let renewable = match home {
        Some(home) => tokio::fs::read_to_string(home.join(AUTH_FILE))
            .await
            .ok()
            .and_then(|text| serde_json::from_str::<Json>(&text).ok())
            .and_then(|file| {
                file.get("tokens")?
                    .get("refresh_token")?
                    .as_str()
                    .map(|token| !token.is_empty())
            })
            .unwrap_or(false),
        None => false,
    };
    if renewable {
        rpc.call_within(
            "account/read",
            json!({"refreshToken":true}),
            ACCOUNT_TIMEOUT,
        )
        .await
        .map_err(|error| AdapterError::HarnessError(remote_text(&error)))?;
    }
    read_facts(rpc, tracker, home, clock).await
}

/// Reads the provider's current identity, credential lifetime and vendor usage without forcing renewal.
pub async fn read_facts(
    rpc: &Rpc,
    tracker: &LoginTracker,
    home: Option<&Path>,
    clock: &dyn Clock,
) -> Result<ProviderFacts, AdapterError> {
    let material = credential_material(home);
    let mode = auth_mode(rpc).await?;
    if mode.is_none() {
        tracker.signed_out();
        return Ok(ProviderFacts::default());
    }
    if let Some(home) = home {
        let (method, expires_at) = read_login_note(home);
        tracker.noted(method, expires_at);
        tracker.issued(read_issued_at(home));
        read_native_lifetime(home, tracker);
    }
    let summary = home.and_then(read_auth_summary).unwrap_or_default();
    let mut identity_observed = summary.email.is_some();
    tracker.signed_in_as(summary.email, summary.plan);
    if let Ok(account) = rpc
        .call_within(
            "account/read",
            json!({ "refreshToken": false }),
            ACCOUNT_TIMEOUT,
        )
        .await
        && let Some(account) = account.get("account").filter(|account| account.is_object())
    {
        let email = account
            .get("email")
            .and_then(Json::as_str)
            .map(str::to_owned);
        let plan = account
            .get("planType")
            .and_then(Json::as_str)
            .map(str::to_owned);
        identity_observed |= email.is_some();
        let (known_email, known_plan, _, _, _) = tracker.snapshot();
        tracker.signed_in_as(email.or(known_email), plan.or(known_plan));
    }
    let read = rpc
        .call_within("account/rateLimits/read", json!({}), ACCOUNT_TIMEOUT)
        .await
        .ok();
    let snapshot = read
        .as_ref()
        .and_then(|read| read.get("rateLimits"))
        .filter(|snapshot| {
            snapshot.is_object()
                && [
                    "planType",
                    "primary",
                    "secondary",
                    "credits",
                    "rateLimitReachedType",
                ]
                .iter()
                .any(|key| snapshot.get(*key).is_some_and(|value| !value.is_null()))
                && ["primary", "secondary"].iter().all(|key| {
                    snapshot.get(*key).is_none_or(|window| {
                        window.is_null()
                            || window
                                .get("usedPercent")
                                .and_then(Json::as_f64)
                                .is_some_and(|used| used.is_finite() && used >= 0.0)
                    })
                })
        });
    if credential_material(home) != material {
        return Err(not_told());
    }
    let Some(snapshot) = snapshot else {
        if let Some(mut previous) = tracker.cached() {
            let local = facts_from_snapshot(&json!({}), tracker, clock.time());
            if previous.credential == local.credential
                && previous.identity == local.identity
                && previous.plan == local.plan
            {
                return Err(not_told());
            }
            // A newly imported credential has a locally known lifetime even if usage was not told.
            previous.credential = local.credential;
            previous.credential_verified = Some(false);
            previous.identity = local.identity;
            previous.plan = local.plan;
            for field in ["credential", "identity", "plan"] {
                previous.field_times.insert(field.to_owned(), clock.time());
            }
            return Ok(tracker.cache(previous));
        }
        // Identity and the imported credential are known locally even before a vendor has ever answered.
        let now = clock.time();
        let mut local = facts_from_snapshot(&json!({}), tracker, now);
        local.credential_verified = Some(false);
        for (field, known) in [
            ("plan", local.plan.is_some()),
            ("identity", local.identity.is_some()),
            ("credential", local.credential.is_some()),
        ] {
            if known {
                local.field_times.insert(field.to_owned(), now);
            }
        }
        return Ok(tracker.cache(local));
    };
    let now = clock.time();
    let previous = tracker.cached().unwrap_or_default();
    let mut facts = facts_from_snapshot(snapshot, tracker, now);
    let mut resets_observed = false;
    facts.field_times = previous.field_times.clone();
    facts.credential_verified = Some(true);
    facts
        .field_times
        .insert("credential_verified".to_owned(), now);
    if identity_observed {
        facts.field_times.insert("identity".to_owned(), now);
    }
    if home.is_some() || tracker.snapshot().2.is_some() {
        facts.field_times.insert("credential".to_owned(), now);
    }
    for (field, native) in [
        ("plan", "planType"),
        ("plan_credits", "credits"),
        ("limit_reached", "rateLimitReachedType"),
    ] {
        if snapshot.get(native).is_some_and(|value| !value.is_null())
            || (field == "limit_reached" && snapshot.get(native).is_some())
        {
            facts.field_times.insert(field.to_owned(), now);
        }
    }
    if snapshot.get("rateLimitReachedType").is_none() {
        facts.limit_reached = previous.limit_reached;
    }
    if facts.plan_credits.is_none() {
        facts.plan_credits = previous.plan_credits;
    }
    if let Some(read) = read.as_ref() {
        facts.available_resets = read.get("rateLimitResetCredits").and_then(resets_from);
        resets_observed = facts.available_resets.is_some();
        if facts.available_resets.is_some() {
            facts.field_times.insert("available_resets".to_owned(), now);
        } else {
            facts.available_resets = previous.available_resets;
        }
        facts.ordinary_usage_allowed = read.get("ordinaryUsageAllowed").and_then(Json::as_bool);
        if facts.ordinary_usage_allowed.is_some() {
            facts
                .field_times
                .insert("ordinary_usage_allowed".to_owned(), now);
        } else {
            facts.ordinary_usage_allowed = previous.ordinary_usage_allowed;
        }
        if let Some(limits) = read.get("rateLimitsByLimitId").and_then(Json::as_object) {
            for (limit, additional) in limits {
                if additional == snapshot
                    || snapshot.get("limitId").and_then(Json::as_str) == Some(limit.as_str())
                {
                    continue;
                }
                for mut window in facts_from_snapshot(additional, tracker, now).windows {
                    window.id = format!("{limit}:{}", window.id);
                    facts.windows.push(window);
                }
            }
        }
    }
    if !facts.windows.is_empty() {
        facts.field_times.insert("windows".to_owned(), now);
    }
    for window in &facts.windows {
        facts
            .field_times
            .insert(format!("windows.{}", window.id), now);
    }
    // An absent source window is still the earlier dated observation, never an invented empty window.
    for window in previous.windows {
        if !facts.windows.iter().any(|current| current.id == window.id) {
            facts.windows.push(window);
        }
    }
    let mut observed = tracker.cache(facts);
    if !resets_observed {
        // The cache remains dated display knowledge; an act may use only this read's actual reset group.
        observed.available_resets = None;
        observed.field_times.remove("available_resets");
    }
    Ok(observed)
}

// Comparing private material is local to this runtime; only its digest participates in the read guard.
fn credential_material(home: Option<&Path>) -> Option<orch_types::Hash> {
    home.and_then(|home| std::fs::read(home.join(AUTH_FILE)).ok())
        .map(|bytes| orch_types::Hash::of(&bytes))
}

fn read_native_lifetime(home: &Path, tracker: &LoginTracker) {
    let Some(file) = std::fs::read_to_string(home.join(AUTH_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Json>(&text).ok())
    else {
        return;
    };
    let Some(tokens) = file.get("tokens") else {
        return;
    };
    if tokens
        .get("refresh_token")
        .and_then(Json::as_str)
        .is_some_and(|value| !value.is_empty())
    {
        if tracker.snapshot().2.as_deref() == Some(method::ACCESS_TOKEN) {
            tracker.noted(None, None);
            tracker.issued(None);
        }
        return;
    }
    if let Some(claims) = tokens
        .get("access_token")
        .and_then(Json::as_str)
        .and_then(claims_of)
        && claims
            .get("https://api.openai.com/auth")
            .and_then(|auth| auth.get("chatgpt_account_id"))
            .is_some()
    {
        tracker.noted(
            Some(method::ACCESS_TOKEN.to_owned()),
            epoch(claims.get("exp")),
        );
        tracker.issued(epoch(claims.get("iat")));
    }
}

/// A vendor did not supply a known observation; the connector retains facts and their time.
#[must_use]
pub fn not_told() -> AdapterError {
    AdapterError::Other {
        code: "facts_not_told".to_owned(),
        message: "The vendor did not tell these facts.".to_owned(),
        retryable: false,
    }
}

/// The resets from the stable account read, including a count without a list.
#[must_use]
pub fn resets_from(value: &Json) -> Option<AvailableResets> {
    let available_count = value
        .get("availableCount")?
        .as_i64()
        .filter(|count| *count >= 0)?;
    let credits = match value.get("credits") {
        None | Some(Json::Null) => None,
        Some(Json::Array(credits)) => Some(
            credits
                .iter()
                .map(|credit| {
                    Some(ResetCredit {
                        id: credit.get("id")?.as_str()?.to_owned(),
                        title: credit
                            .get("title")
                            .and_then(Json::as_str)
                            .map(str::to_owned),
                        description: credit
                            .get("description")
                            .and_then(Json::as_str)
                            .map(str::to_owned),
                        reset_type: credit.get("resetType")?.as_str()?.to_owned(),
                        status: credit.get("status")?.as_str()?.to_owned(),
                        granted_at: epoch(credit.get("grantedAt")),
                        expires_at: epoch(credit.get("expiresAt")),
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        ),
        _ => return None,
    };
    Some(AvailableResets {
        available_count,
        credits,
    })
}

/// Uses a reset through the stable app-server interface, preserving the same key on every retry.
pub async fn consume_reset(
    rpc: &Rpc,
    tracker: &LoginTracker,
    home: Option<&Path>,
    clock: &dyn Clock,
    request: ResetRequest,
) -> Result<ResetResult, AdapterError> {
    let answer = rpc
        .call_within(
            "account/rateLimitResetCredit/consume",
            json!({
                "creditId": request.reset_id, "idempotencyKey": request.idempotency_key
            }),
            ACCOUNT_TIMEOUT,
        )
        .await
        .map_err(|error| AdapterError::HarnessError(remote_text(&error)))?;
    let outcome = answer
        .get("outcome")
        .and_then(Json::as_str)
        .ok_or_else(not_told)?
        .to_owned();
    let facts = match read_facts(rpc, tracker, home, clock).await {
        Ok(facts) => facts,
        Err(error) if error.code() == "facts_not_told" => {
            let mut cached = tracker.cached().unwrap_or_default();
            cached.credential_verified = None;
            cached
        }
        Err(error) => return Err(error),
    };
    Ok(ResetResult { outcome, facts })
}

// ---------------------------------------------------------------------------------------------------------
// Models

/// The model a catalogue entry of the harness is, as a suggestion.
#[must_use]
pub fn suggestion_of(model: &Json) -> Option<ModelSuggestion> {
    let name = model
        .get("model")
        .and_then(Json::as_str)
        .or_else(|| model.get("id").and_then(Json::as_str))?;
    let display = model.get("displayName").and_then(Json::as_str);
    let efforts: Vec<String> = model
        .get("supportedReasoningEfforts")
        .and_then(Json::as_array)
        .map(|efforts| {
            efforts
                .iter()
                .filter_map(|effort| effort.get("reasoningEffort").and_then(Json::as_str))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let takes_images = model
        .get("inputModalities")
        .and_then(Json::as_array)
        .map(|modalities| {
            modalities
                .iter()
                .any(|modality| modality.as_str() == Some("image"))
        });
    let mut suggestion = ModelSuggestion::named(name, display);
    suggestion.parameters = ModelParameters {
        efforts: (!efforts.is_empty()).then_some(efforts),
        default_effort: model
            .get("defaultReasoningEffort")
            .and_then(Json::as_str)
            .map(str::to_owned),
        // What a thread starts with of a speed or a program is a manager's choice, and so is a larger
        // context; the catalogue's own start for a speed (`defaultServiceTier`) is left out on purpose.
        context_window: window_of(model),
        offered: offered_for(model),
        input_images: takes_images,
        ..ModelParameters::default()
    };
    Some(suggestion)
}

/// The models the harness lists, hidden ones left out.
pub async fn list_models(rpc: &Rpc) -> Result<Vec<ModelSuggestion>, AdapterError> {
    let mut found = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = rpc
            .call(
                "model/list",
                json!({ "includeHidden": false, "cursor": cursor, "limit": 100 }),
            )
            .await
            .map_err(|error| AdapterError::HarnessError(remote_text(&error)))?;
        if let Some(models) = page.get("data").and_then(Json::as_array) {
            found.extend(
                models
                    .iter()
                    .filter(|model| model.get("hidden").and_then(Json::as_bool) != Some(true))
                    .filter_map(suggestion_of),
            );
        }
        cursor = page
            .get("nextCursor")
            .and_then(Json::as_str)
            .map(str::to_owned);
        if cursor.is_none() {
            return Ok(found);
        }
    }
}

/// What the catalogue says of the window of one model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogueWindow {
    /// The window the harness works with when it is told nothing.
    pub window: i64,
    /// The largest an override of the window may set, where that is more than the window: the harness holds a
    /// larger one to it.
    pub largest: Option<i64>,
}

/// The windows a catalogue gives, by the name each model is run under.
#[must_use]
pub fn windows_of_catalogue(catalogue: &Json) -> BTreeMap<String, CatalogueWindow> {
    let Some(models) = catalogue.get("models").and_then(Json::as_array) else {
        return BTreeMap::new();
    };
    models
        .iter()
        .filter_map(|model| {
            let name = model.get("slug").and_then(Json::as_str)?;
            let window = model
                .get("context_window")
                .and_then(Json::as_i64)
                .filter(|window| *window > 0)?;
            let largest = model
                .get("max_context_window")
                .and_then(Json::as_i64)
                .filter(|largest| *largest > window);
            Some((name.to_owned(), CatalogueWindow { window, largest }))
        })
        .collect()
}

/// Gives the suggestions that came without a window the one the catalogue has for them, and, where the
/// catalogue lets an override set more, the largest as what the model *offers*: it is a manager who opens it.
/// What the list itself said is kept.
pub fn add_windows(found: &mut [ModelSuggestion], windows: &BTreeMap<String, CatalogueWindow>) {
    for suggestion in found {
        let Some(known) = windows.get(&suggestion.model_name) else {
            continue;
        };
        let parameters = &mut suggestion.parameters;
        let window = *parameters.context_window.get_or_insert(known.window);
        if let Some(largest) = known.largest.filter(|largest| *largest > window) {
            let offered = parameters.offered.get_or_insert_with(Offered::default);
            offered.context_max.get_or_insert(largest);
        }
    }
}

/// Asks the harness to render its catalogue of models and reads the windows in it. Nothing is read where the
/// command is not there, fails, takes longer than [`CATALOGUE_TIMEOUT`] or says something else: a list without
/// windows is what discovery gave before, and a manager can write them.
pub async fn read_windows(
    launcher: &dyn Launcher,
    program: &str,
    env: &BTreeMap<String, String>,
) -> BTreeMap<String, CatalogueWindow> {
    let work = async {
        let mut spawned = launcher
            .launch(LaunchRequest {
                program: program.to_owned(),
                args: CATALOGUE_COMMAND.map(str::to_owned).to_vec(),
                env: env.clone(),
                cwd: None,
            })
            .await
            .ok()?;
        // It is given nothing to read, and what it writes for the log is read to the end and dropped, so that it
        // never waits for someone to do it.
        drop(std::mem::replace(
            &mut spawned.stdin,
            Box::new(tokio::io::sink()),
        ));
        if let Some(mut log) = spawned.stderr.take() {
            tokio::spawn(async move {
                let _ = tokio::io::copy(&mut log, &mut tokio::io::sink()).await;
            });
        }
        let mut bytes = Vec::new();
        (&mut spawned.stdout)
            .take(CATALOGUE_MAX_BYTES)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        let ended = spawned.control.wait().await;
        (ended.code == Some(0)).then_some(bytes)
    };
    let Ok(Some(bytes)) = tokio::time::timeout(CATALOGUE_TIMEOUT, work).await else {
        return BTreeMap::new();
    };
    serde_json::from_slice::<Json>(&bytes)
        .map(|catalogue| windows_of_catalogue(&catalogue))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::login::check_methods;

    #[test]
    fn the_initialize_wait_defaults_to_120_seconds_and_reads_the_provider_setting() {
        assert_eq!(
            initialize_timeout(&json!({})).unwrap(),
            Duration::from_secs(120)
        );
        assert_eq!(
            options_schema()["properties"]["initialize_timeout_secs"]["default"],
            120
        );
        for seconds in [1, 240, 3600] {
            assert_eq!(
                initialize_timeout(&json!({ "initialize_timeout_secs": seconds })).unwrap(),
                Duration::from_secs(seconds)
            );
        }
    }

    #[test]
    fn an_invalid_initialize_wait_is_refused_instead_of_panicking_or_waiting_forever() {
        for value in [
            json!(0),
            json!(-1),
            json!(0.5),
            json!("120"),
            Json::Null,
            json!(u64::MAX),
        ] {
            let error =
                initialize_timeout(&json!({ "initialize_timeout_secs": value })).unwrap_err();
            assert_eq!(error.code(), "invalid_input");
            assert!(error.to_string().contains("initialize_timeout_secs"));
        }
    }

    fn fake_auth_json() -> String {
        let part = |value: &Json| URL_SAFE_NO_PAD.encode(value.to_string());
        let id_token = format!(
            "{}.{}.{}",
            part(&json!({ "alg": "RS256" })),
            part(&json!({
                "email": "dana@example.com",
                "https://api.openai.com/auth": { "chatgpt_plan_type": "team", "chatgpt_account_id": "acct-1" }
            })),
            part(&json!("signature")),
        );
        json!({
            "auth_mode": "chatgpt", "OPENAI_API_KEY": null,
            "tokens": { "id_token": id_token, "access_token": "at", "refresh_token": "rt", "account_id": "acct-1" },
            "last_refresh": "2026-10-11T10:00:00Z"
        })
        .to_string()
    }

    #[test]
    fn the_three_ways_to_sign_in_are_declared_and_fit_together() {
        let methods = login_methods();
        assert_eq!(check_methods(&methods), Ok(()));
        let ids: Vec<_> = methods.iter().map(|method| method.id.as_str()).collect();
        assert_eq!(ids, ["device_code", "auth_file", "access_token"]);
        assert!(methods[0].recommended && methods[0].requires.is_some());
        assert_eq!(methods[0].kind, LoginKind::Interactive);
        assert_eq!(methods[1].fields.len(), 2, "the contents or the path");
        assert_eq!(methods[1].fields[0].max_bytes, Some(65_536));
        assert_eq!(methods[2].yields, LoginYields::Token);
        assert_eq!(methods[2].fields[1].kind, FieldType::Date);
        assert!(methods[1].summary.contains("one copy in one place"));
    }

    #[test]
    fn an_auth_file_is_checked_and_says_who_it_is_for() {
        let summary = check_auth_file(&fake_auth_json()).unwrap();
        assert_eq!(summary.email.as_deref(), Some("dana@example.com"));
        assert_eq!(summary.plan.as_deref(), Some("team"));
        assert_eq!(summary.account_id.as_deref(), Some("acct-1"));
        let key = check_auth_file(r#"{ "OPENAI_API_KEY": "sk-abc" }"#).unwrap();
        assert_eq!(key, AuthSummary::default());
    }

    #[test]
    fn what_is_not_an_auth_file_is_refused_without_quoting_it() {
        for bad in [
            "not json at all with a-secret-in-it",
            "[1, 2]",
            r#"{ "tokens": {} }"#,
            r#"{ "tokens": { "access_token": "a-secret" } }"#,
            r#"{ "OPENAI_API_KEY": "" }"#,
        ] {
            let error = check_auth_file(bad).unwrap_err();
            assert_eq!(error.code(), "invalid_login_values", "{bad}");
            assert!(!error.to_string().contains("a-secret"), "{error}");
        }
        let huge = format!(r#"{{ "OPENAI_API_KEY": "{}" }}"#, "k".repeat(70_000));
        assert!(check_auth_file(&huge).is_err());
    }

    #[test]
    fn a_window_of_the_plan_is_named_by_its_length() {
        assert_eq!(window_label(Some(300)), "5h");
        assert_eq!(window_label(Some(10_080)), "weekly");
        assert_eq!(window_label(Some(1_440)), "1d");
        assert_eq!(window_label(Some(4_320)), "3d");
        assert_eq!(window_label(Some(45)), "45m");
        assert_eq!(window_label(None), "window");
    }

    #[test]
    fn the_limits_of_a_plan_become_the_facts_of_the_provider() {
        let tracker = LoginTracker::default();
        tracker.signed_in_as(Some("dana@example.com".into()), None);
        tracker.noted(Some("device_code".into()), None);
        let facts = facts_from_snapshot(
            &json!({
                "planType": "pro",
                "primary": { "usedPercent": 37.5, "windowDurationMins": 300, "resetsAt": 1_791_730_800 },
                "secondary": { "usedPercent": 12, "windowDurationMins": 10_080, "resetsAt": null },
                "credits": { "hasCredits": true, "unlimited": false, "balance": "1250" },
                "rateLimitReachedType": null
            }),
            &tracker,
            "2026-10-11T10:00:00.000Z".parse().unwrap(),
        );
        assert_eq!(facts.plan.as_deref(), Some("pro"));
        assert_eq!(facts.windows.len(), 2);
        assert_eq!(facts.windows[0].label, "5h");
        assert!((facts.windows[0].used_percent - 37.5).abs() < f64::EPSILON);
        assert!(facts.windows[0].resets_at.is_some() && facts.windows[1].resets_at.is_none());
        assert_eq!(
            facts.plan_credits.as_ref().unwrap().balance.as_deref(),
            Some("1250")
        );
        assert_eq!(facts.identity.as_deref(), Some("dana@example.com"));
        assert_eq!(
            facts.credential.as_ref().unwrap().method.as_deref(),
            Some("device_code")
        );
        assert!(!facts.is_exhausted());
        let limited = facts_from_snapshot(
            &json!({ "primary": { "usedPercent": 100, "windowDurationMins": 300, "resetsAt": 1_791_730_800 },
                     "rateLimitReachedType": "workspace_member_usage_limit_reached" }),
            &tracker,
            "2026-10-11T10:00:00.000Z".parse().unwrap(),
        );
        assert_eq!(
            limited.limit_reached.as_ref().unwrap().kind,
            "workspace_member_usage_limit_reached"
        );
        assert!(limited.is_exhausted());
    }

    #[test]
    fn an_update_that_says_nothing_is_no_fact_and_a_sign_out_clears_them() {
        let tracker = LoginTracker::default();
        let empty = json!({ "rateLimits": { "limitId": "codex", "primary": null, "secondary": null, "credits": null, "planType": null, "rateLimitReachedType": null } });
        assert!(
            facts_from_update(
                "account/rateLimits/updated",
                &empty,
                &tracker,
                "2026-10-11T10:00:00.000Z".parse().unwrap()
            )
            .is_none()
        );
        tracker.signed_in_as(Some("a@b".into()), Some("plus".into()));
        let out = facts_from_update(
            "account/updated",
            &json!({ "authMode": null, "planType": null }),
            &tracker,
            "2026-10-11T10:00:00.000Z".parse().unwrap(),
        )
        .unwrap();
        assert_eq!(out, ProviderFacts::default());
        assert!(
            facts_from_update(
                "account/updated",
                &json!({ "authMode": "chatgpt", "planType": "pro" }),
                &tracker,
                "2026-10-11T10:00:00.000Z".parse().unwrap(),
            )
            .is_some_and(|facts| facts.plan.as_deref() == Some("pro"))
        );
    }

    #[test]
    fn a_device_code_sign_in_shows_an_address_and_a_code_and_ends_as_the_harness_says() {
        let tracker = LoginTracker::default();
        let now: Time = "2026-10-11T10:00:00.000Z".parse().unwrap();
        let state = device_code_state(
            &json!({ "type": "chatgptDeviceCode", "loginId": "l1", "verificationUrl": "https://example.test/device", "userCode": "ABCD-1234" }),
            now,
        );
        assert_eq!(state.state, LoginStateKind::AwaitingUser);
        assert_eq!(state.shows[0].kind, ShowKind::Uri);
        assert_eq!(state.shows[1].value, "ABCD-1234");
        assert_eq!(
            state.expires_at.unwrap().to_string(),
            "2026-10-11T10:15:00.000Z"
        );
        tracker.started("l1", "device_code");
        assert!(
            tracker
                .completed(&json!({ "loginId": "other", "success": true }))
                .is_none()
        );
        assert_eq!(tracker.login_id().as_deref(), Some("l1"));
        let done = tracker
            .completed(&json!({ "loginId": "l1", "success": true, "error": null }))
            .unwrap();
        assert_eq!(done.state, LoginStateKind::Completed);
        tracker.started("l2", "device_code");
        let failed = tracker
            .completed(&json!({ "loginId": "l2", "success": false, "error": "expired" }))
            .unwrap();
        assert_eq!(
            (failed.state, failed.error.as_deref()),
            (LoginStateKind::Failed, Some("expired"))
        );
    }

    #[test]
    fn a_model_of_the_catalogue_is_a_suggestion_with_what_it_offers_and_nothing_enabled() {
        let suggestion = suggestion_of(&json!({
            "id": "gpt-6-sol", "model": "gpt-6-sol", "displayName": "GPT-6-Sol", "hidden": false,
            "supportedReasoningEfforts": [ { "reasoningEffort": "low", "description": "" }, { "reasoningEffort": "high", "description": "" } ],
            "defaultReasoningEffort": "low", "inputModalities": ["text", "image"],
            "serviceTiers": [ { "id": "priority", "name": "Fast", "description": "2x speed" } ],
            "availableAccessPrograms": null, "isDefault": false
        }))
        .unwrap();
        assert_eq!(suggestion.model_name, "gpt-6-sol");
        assert_eq!(suggestion.display_name, "GPT-6-Sol");
        assert_eq!(suggestion.parameters.efforts(), ["low", "high"]);
        assert_eq!(suggestion.parameters.default_effort.as_deref(), Some("low"));
        assert!(suggestion.parameters.takes_images());
        assert!(
            suggestion.parameters.service_tiers().is_empty(),
            "a speed is offered, never enabled"
        );
        assert_eq!(
            suggestion
                .parameters
                .offered
                .as_ref()
                .unwrap()
                .service_tiers
                .as_ref()
                .unwrap()[0]
                .id,
            "priority"
        );
    }

    #[test]
    fn how_a_home_was_signed_in_is_noted_and_read_back() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(read_login_note(home.path()), (None, None));
        let lapses: Time = "2027-01-01T00:00:00.000Z".parse().unwrap();
        write_login_note(home.path(), "access_token", Some(lapses)).unwrap();
        assert_eq!(
            read_login_note(home.path()),
            (Some("access_token".into()), Some(lapses))
        );
    }

    fn catalogue() -> Json {
        json!({ "models": [
            { "slug": "gpt-6-sol", "context_window": 272_000, "max_context_window": 872_000,
              "effective_context_window_percent": 95 },
            { "slug": "gpt-5.5", "context_window": 272_000, "max_context_window": 272_000 },
            { "slug": "no-window", "display_name": "A model without a window" },
            { "context_window": 1, "max_context_window": 2 }
        ] })
    }

    #[test]
    fn the_windows_the_catalogue_gives_are_the_one_a_model_starts_with_and_the_largest_it_takes_where_that_is_more()
     {
        let windows = windows_of_catalogue(&catalogue());
        assert_eq!(
            windows.get("gpt-6-sol"),
            Some(&CatalogueWindow {
                window: 272_000,
                largest: Some(872_000)
            })
        );
        assert_eq!(
            windows.get("gpt-5.5"),
            Some(&CatalogueWindow {
                window: 272_000,
                largest: None
            }),
            "a largest that is no more than the window is not one"
        );
        assert_eq!(
            windows.len(),
            2,
            "a model without a window, or a name, says nothing"
        );
        assert!(windows_of_catalogue(&json!({ "models": "none" })).is_empty());
        assert!(windows_of_catalogue(&json!([])).is_empty());
    }

    #[test]
    fn a_list_without_windows_is_given_them_from_the_catalogue_and_what_it_said_itself_is_kept() {
        let mut windows = windows_of_catalogue(&catalogue());
        let mut found: Vec<ModelSuggestion> = ["gpt-6-sol", "gpt-5.5", "other", "told"]
            .iter()
            .filter_map(|name| suggestion_of(&json!({ "model": name })))
            .collect();
        // A list that did give the window of a model, and the largest, is not corrected by the catalogue.
        found[3].parameters.context_window = Some(100_000);
        found[3].parameters.offered = Some(Offered {
            context_max: Some(300_000),
            ..Offered::default()
        });
        windows.insert(
            "told".to_owned(),
            CatalogueWindow {
                window: 272_000,
                largest: Some(872_000),
            },
        );
        add_windows(&mut found, &windows);
        assert_eq!(found[0].parameters.context_window, Some(272_000));
        assert_eq!(
            found[0]
                .parameters
                .offered
                .as_ref()
                .and_then(|o| o.context_max),
            Some(872_000),
            "the largest is on offer and not open"
        );
        assert_eq!(found[0].parameters.context_max, None);
        assert_eq!(found[1].parameters.context_window, Some(272_000));
        assert_eq!(found[1].parameters.offered, None, "nothing larger to offer");
        assert_eq!(
            found[2].parameters.context_window, None,
            "a model the catalogue lacks"
        );
        assert_eq!(found[3].parameters.context_window, Some(100_000));
        assert_eq!(
            found[3]
                .parameters
                .offered
                .as_ref()
                .and_then(|o| o.context_max),
            Some(300_000)
        );
    }
}
