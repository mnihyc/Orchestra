//! Sign-in on the provider (a pasted long-lived token, a browser code), login facts, shared tokens and the handed-over login.
//!
//! A subscription is signed in on the provider's runtime, in the harness's home, in one of two ways. The
//! long-lived token that the vendor's `setup-token` command prints is pasted in and kept in a file of this
//! adapter's own (mode 0600) in the home, and every process of the harness is started with it in its
//! environment. The browser code is the harness's own sign-in: it prints an address, waits for the code that
//! page gives, and keeps the login it gets in its own file, which it renews by itself.
//!
//! What can leave the provider's runtime is exactly what the descriptor lists: the long-lived token as it is,
//! a short-lived access token read from the stored login without the token that renews it, and the stored
//! login whole (which moves, and is not used here until it is returned).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::parameters::FAST_TIER;
use orch_agent_contract::adapter::{AdapterError, AdapterEvent, EventSink};
use orch_agent_contract::credentials::{GivenCredential, IssuedCredential};
use orch_agent_contract::descriptor::ProviderFactKind;
use orch_agent_contract::discovery::ModelSuggestion;
use orch_agent_contract::facts::{
    CredentialFacts, CredentialYield, ExtraUsage, LimitReached, ProviderFacts, UsageWindow,
};
use orch_agent_contract::flow;
use orch_agent_contract::login::{LoginShow, LoginState, LoginStateKind, ShowKind};
use orch_agent_contract::parameters::{ModelParameters, Offered, ServiceTier};
use orch_plugin_contract::Secret;
use orch_types::{Clock, Field, FieldType, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{Notify, watch};

use crate::config::PROBE_TIMEOUT;
use crate::descriptor::{
    FORM_ACCESS_TOKEN, FORM_LOGIN, FORM_OAUTH_TOKEN, METHOD_BROWSER_CODE, METHOD_SETUP_TOKEN,
};
use crate::process::{LaunchSpec, Launcher, ProcessEvent, base_environment, run_to_end, spawn};
use crate::sessions::HarnessSetup;
use crate::stream::{control_request, encode_line};

/// The directory of the home in which this adapter keeps what is its own.
pub const OWN_DIRECTORY: &str = ".orch";
/// The file in which the long-lived token is kept.
pub const TOKEN_FILE: &str = "oauth.json";
/// The file in which the harness keeps the login it renews.
pub const LOGIN_FILE: &str = ".credentials.json";
/// The file in which a login that was handed over waits, unused, until it is returned.
pub const HELD_LOGIN_FILE: &str = "held-login.json";

/// How long a long-lived token is good for, by the vendor's documentation: a year.
const TOKEN_LIFETIME_SECS: i64 = 31_536_000;

/// How long a person is given to come back with the code of the sign-in page.
pub const CODE_WAIT_SECS: i64 = 600;

/// How much life an access token must have left to be handed on without asking the harness to renew it.
const ACCESS_TOKEN_MARGIN_MILLIS: i64 = 5 * 60 * 1000;

/// The long-lived token as it is kept.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredToken {
    /// The token.
    pub token: String,
    /// Which sign-in method brought it.
    pub method: String,
    /// When it was stored.
    pub stored_at: Time,
}

impl StoredToken {
    /// When it lapses.
    #[must_use]
    pub fn expires_at(&self) -> Time {
        Time::new(self.stored_at.get() + chrono::Duration::seconds(TOKEN_LIFETIME_SECS))
    }
}

fn own_file(home: &Path, name: &str) -> PathBuf {
    home.join(OWN_DIRECTORY).join(name)
}

/// Writes a file that only the user this runs as may read.
async fn write_private(path: &Path, content: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, content).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(())
}

/// The token kept in a home, if there is one.
pub async fn stored_token(home: &Path) -> Option<StoredToken> {
    let text = tokio::fs::read_to_string(own_file(home, TOKEN_FILE))
        .await
        .ok()?;
    serde_json::from_str(&text).ok()
}

/// Keeps a token.
pub async fn store_token(
    home: &Path,
    token: &str,
    method: &str,
    now: Time,
) -> Result<StoredToken, AdapterError> {
    let stored = StoredToken {
        token: token.trim().to_owned(),
        method: method.to_owned(),
        stored_at: now,
    };
    let text = serde_json::to_string(&stored)
        .map_err(|error| AdapterError::Internal(error.to_string()))?;
    write_private(&own_file(home, TOKEN_FILE), text.as_bytes())
        .await
        .map_err(|error| AdapterError::Internal(format!("the token could not be kept: {error}")))?;
    Ok(stored)
}

/// Forgets the token.
pub async fn forget_token(home: &Path) {
    let _ = tokio::fs::remove_file(own_file(home, TOKEN_FILE)).await;
}

/// Whether a text can be a token: one word with no space or control character.
#[must_use]
pub fn looks_like_a_token(text: &str) -> bool {
    let token = text.trim();
    !token.is_empty()
        && token.len() <= 4096
        && token.chars().all(|c| !c.is_whitespace() && !c.is_control())
}

// ---------------------------------------------------------------------------------------------------------
// Asking the harness

/// The environment of a question to the harness about its sign-in.
fn auth_environment(
    setup: &HarnessSetup,
    token: Option<&str>,
) -> std::collections::BTreeMap<String, String> {
    let mut env = base_environment(setup.inherited.clone());
    env.extend(setup.profile_env.clone());
    if setup.managed_home {
        env.extend(crate::sessions::managed_environment(false));
    }
    crate::sessions::place_home(&mut env, setup);
    if let Some(token) = token {
        env.insert("CLAUDE_CODE_OAUTH_TOKEN".to_owned(), token.to_owned());
    }
    env
}

fn auth_spec(setup: &HarnessSetup, token: Option<&str>, args: &[&str]) -> LaunchSpec {
    LaunchSpec {
        program: setup.program.clone(),
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        env: auth_environment(setup, token),
        cwd: std::env::temp_dir(),
        label: "auth".to_owned(),
    }
}

/// Disposable print sessions obey the same managed settings as ordinary conversations.
fn probe_spec(setup: &HarnessSetup, token: Option<&str>, label: &str) -> LaunchSpec {
    let mut env = auth_environment(setup, token);
    env.insert(
        "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".to_owned(),
        "1".to_owned(),
    );
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--strict-mcp-config",
        "--setting-sources",
        "",
        "--no-session-persistence",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if setup.managed_home {
        args.extend([
            "--settings".to_owned(),
            Json::Object(crate::sessions::managed_settings(false)).to_string(),
        ]);
    }
    LaunchSpec {
        program: setup.program.clone(),
        args,
        env,
        cwd: std::env::temp_dir(),
        label: label.to_owned(),
    }
}

/// What the harness says of its sign-in (`auth status`, which prints JSON).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AuthStatus {
    /// Whether a sign-in is there.
    pub logged_in: bool,
    /// How: the harness's own name for it.
    pub auth_method: Option<String>,
    /// Who, when it says.
    pub email: Option<String>,
    /// The plan, when it says.
    pub plan: Option<String>,
    /// The organization, when it says.
    pub organization: Option<String>,
}

impl AuthStatus {
    /// Reads the JSON that `auth status` prints.
    #[must_use]
    pub fn from_json(value: &Json) -> Self {
        let text = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| value.get(*name).and_then(Json::as_str))
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        Self {
            logged_in: value
                .get("loggedIn")
                .and_then(Json::as_bool)
                .unwrap_or(false),
            auth_method: text(&["authMethod"]).filter(|method| method != "none"),
            email: text(&["email", "account"]),
            plan: text(&["subscriptionType", "plan"]).map(|plan| plan.to_ascii_lowercase()),
            organization: text(&["orgName", "organization", "organizationName"]),
        }
    }
}

/// Asks the harness about its sign-in.
pub async fn auth_status(
    launcher: &dyn Launcher,
    setup: &HarnessSetup,
    token: Option<&str>,
) -> Result<AuthStatus, AdapterError> {
    let spec = auth_spec(setup, token, &["auth", "status", "--json"]);
    let finished = run_to_end(launcher, &spec, PROBE_TIMEOUT)
        .await
        .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
    let printed = finished.lines.join("\n");
    let value: Json = serde_json::from_str(&printed).map_err(|_| {
        AdapterError::HarnessError(format!(
            "the harness did not say how it is signed in ({})",
            finished.exit.describe()
        ))
    })?;
    Ok(AuthStatus::from_json(&value))
}

/// Asks the native harness to use its sign-in, then reads its identity and vendor facts again.
/// Imported tokens stay under the existing read-only token path; an unchanged lapse is reported by the caller.
pub async fn renew(
    launcher: &dyn Launcher,
    setup: &HarnessSetup,
    token: Option<&StoredToken>,
    now: Time,
) -> Result<ProviderFacts, AdapterError> {
    if token.is_none() {
        auth_status(launcher, setup, None).await?;
    }
    facts(launcher, setup, token, now).await
}

/// The facts of the provider: who is signed in, on which plan, and how.
pub async fn facts(
    launcher: &dyn Launcher,
    setup: &HarnessSetup,
    token: Option<&StoredToken>,
    now: Time,
) -> Result<ProviderFacts, AdapterError> {
    let status = auth_status(launcher, setup, token.map(|stored| stored.token.as_str())).await?;
    if !status.logged_in {
        return Err(AdapterError::LoginRequired);
    }
    let credential = match token {
        Some(stored) => CredentialFacts {
            method: Some(stored.method.clone()),
            yields: CredentialYield::Token,
            expires_at: Some(stored.expires_at()),
            issued_at: None,
        },
        None => CredentialFacts {
            method: Some(METHOD_BROWSER_CODE.to_owned()),
            yields: CredentialYield::Login,
            expires_at: None,
            issued_at: None,
        },
    };
    let mut facts = ProviderFacts {
        plan: status.plan,
        windows: Vec::new(),
        plan_credits: None,
        limit_reached: None,
        identity: status.email.or(status.organization),
        credential: Some(credential),
        credential_verified: None,
        ..ProviderFacts::default()
    };
    let home = credential_home(setup);
    if token.is_none()
        && let Some(home) = home.as_deref()
        && let Some(login) = std::fs::read_to_string(home.join(LOGIN_FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<Json>(&text).ok())
        && let Some(oauth) = login.get("claudeAiOauth")
        && !oauth
            .get("refreshToken")
            .and_then(Json::as_str)
            .is_some_and(|value| !value.is_empty())
        && let Some((_, expires)) = access_token_of(&login)
    {
        facts.credential = Some(CredentialFacts {
            method: None,
            yields: CredentialYield::Token,
            issued_at: None,
            expires_at: chrono::DateTime::from_timestamp_millis(expires).map(Time::new),
        });
    }
    let cached = home.as_deref().and_then(cached_facts);
    if let Some(cached) = &cached {
        facts.windows.clone_from(&cached.windows);
        facts.extra_usage.clone_from(&cached.extra_usage);
        facts.limit_reached.clone_from(&cached.limit_reached);
        facts.field_times.clone_from(&cached.field_times);
    }
    facts.field_times.remove("credential_verified");
    if let Some(usage) = native_usage(
        launcher,
        setup,
        token.map(|stored| stored.token.as_str()),
        now,
    )
    .await
    {
        let verified = usage.credential_verified == Some(true);
        if usage.plan.is_some() {
            facts.plan = usage.plan;
        }
        // A missing experimental member leaves the previous observation and its time intact.
        for window in usage.windows {
            if let Some(known) = facts.windows.iter_mut().find(|known| known.id == window.id) {
                *known = window;
            } else {
                facts.windows.push(window);
            }
        }
        if usage.field_times.contains_key("extra_usage") {
            facts.extra_usage = usage.extra_usage;
        }
        facts.credential_verified = usage.credential_verified;
        facts.field_times.extend(usage.field_times);
        if verified {
            for field in ["plan", "identity", "credential"] {
                facts.field_times.insert(field.to_owned(), now);
            }
        }
    }
    for field in ["plan", "identity", "credential"] {
        let value = match field {
            "plan" => facts.plan.is_some(),
            "identity" => facts.identity.is_some(),
            _ => facts.credential.is_some(),
        };
        if value
            && (cached.is_none()
                || cached.as_ref().is_some_and(|previous| {
                    (field == "plan" && previous.plan != facts.plan)
                        || (field == "identity" && previous.identity != facts.identity)
                        || (field == "credential" && previous.credential != facts.credential)
                }))
        {
            facts.field_times.insert(field.to_owned(), now);
        }
    }
    if let Some(home) = home.as_deref() {
        keep_facts(home, &facts);
    }
    Ok(facts)
}

/// The kinds of fact this adapter reports.
#[must_use]
pub fn fact_kinds() -> Vec<ProviderFactKind> {
    vec![
        ProviderFactKind::Plan,
        ProviderFactKind::Windows,
        ProviderFactKind::LimitReached,
        ProviderFactKind::ExtraUsage,
        ProviderFactKind::CredentialVerification,
    ]
}

/// The native read stays within the provider source's eight-second deadline, including initialization.
const PLAN_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
/// Plan data is a few KiB; a MiB bounds an unexpected experimental control response.
const PLAN_REPLY_MAX_BYTES: usize = 1024 * 1024;
/// The vendor's session window is five hours.
const SESSION_MINUTES: i64 = 300;
/// The vendor's week is seven days.
const WEEK_MINUTES: i64 = 7 * 24 * 60;
/// The vendor's warning begins at three quarters of a window; it is displayed and never notified.
const WARNING_PERCENT: f64 = 75.0;
/// The fact cache contains no credential text and survives the harness's processes.
const FACTS_FILE: &str = "provider-facts.json";
/// The model windows learned from results survive until discovery is asked for them.
const MODEL_WINDOWS_FILE: &str = "model-windows.json";

fn credential_home(setup: &HarnessSetup) -> Option<PathBuf> {
    // Read credentials where the actual auth/turn process reads them, including profile/runtime
    // overrides and the selected typed home. The same environment supplier decides both paths.
    let environment = auth_environment(setup, None);
    environment
        .get("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            environment
                .get("HOME")
                .map(|home| Path::new(home).join(".claude"))
        })
}

fn cached_facts(home: &Path) -> Option<ProviderFacts> {
    serde_json::from_str(&std::fs::read_to_string(own_file(home, FACTS_FILE)).ok()?).ok()
}

fn keep_facts(home: &Path, facts: &ProviderFacts) {
    let path = own_file(home, FACTS_FILE);
    if let Ok(text) = serde_json::to_vec(facts) {
        let _ = std::fs::create_dir_all(home.join(OWN_DIRECTORY));
        let _ = std::fs::write(path, text);
    }
}

/// A cancelled or unanswered observation must not leave a waiting harness behind.
struct UsageProbe(crate::process::Running);

impl Drop for UsageProbe {
    fn drop(&mut self) {
        self.0.close_input();
        self.0.kill();
    }
}

/// Asks the unmodified harness for its plan usage without sending a prompt or making an HTTP request.
pub async fn native_usage(
    launcher: &dyn Launcher,
    setup: &HarnessSetup,
    token: Option<&str>,
    now: Time,
) -> Option<ProviderFacts> {
    let spec = probe_spec(setup, token, "usage");
    let (events_tx, mut events) = crate::process::event_channel();
    let probe = UsageProbe(spawn(launcher, &spec, 0, events_tx).ok()?);
    if !probe
        .0
        .write(encode_line(&control_request(
            "usage-initialize",
            json!({"subtype":"initialize"}),
        )))
        .await
    {
        return None;
    }
    tokio::time::timeout(PLAN_READ_TIMEOUT, async {
        let mut initialized = false;
        while let Some((_, event)) = events.recv().await {
            match event {
                ProcessEvent::Line(line) => {
                    if line.len() > PLAN_REPLY_MAX_BYTES {
                        return None;
                    }
                    let Ok(frame) = crate::stream::Frame::parse(&line) else {
                        continue;
                    };
                    if frame.text("type") != Some("control_response") {
                        continue;
                    }
                    let Some((id, outcome)) = frame.control_outcome() else {
                        continue;
                    };
                    if id == "usage-initialize" && !initialized {
                        if !matches!(outcome, crate::stream::ControlOutcome::Success(_)) {
                            return None;
                        }
                        initialized = true;
                        // The pinned harness can omit its expensive local transcript scan for a plan read.
                        if !probe
                            .0
                            .write(encode_line(&control_request(
                                "usage",
                                json!({"subtype":"get_usage","skip_behaviors":true}),
                            )))
                            .await
                        {
                            return None;
                        }
                    } else if id == "usage" && initialized {
                        return match outcome {
                            crate::stream::ControlOutcome::Success(body) => {
                                usage_answer(&body, now)
                            }
                            crate::stream::ControlOutcome::Error(_) => None,
                        };
                    }
                }
                ProcessEvent::Exited(_) | ProcessEvent::TooLong | ProcessEvent::Unreadable(_) => {
                    return None;
                }
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// Reads the experimental get_usage body; session costs and missing plan members never become account facts.
#[must_use]
pub fn usage_answer(body: &Json, now: Time) -> Option<ProviderFacts> {
    let limits = if body.get("rate_limits_available").and_then(Json::as_bool) == Some(true) {
        body.get("rate_limits").filter(|value| value.is_object())
    } else {
        None
    };
    let mut facts = limits.and_then(usage_from).unwrap_or_default();
    if !facts.windows.is_empty() || facts.extra_usage.is_some() {
        facts.credential_verified = Some(true);
        facts
            .field_times
            .insert("credential_verified".to_owned(), now);
    }
    if !facts.windows.is_empty() {
        facts.field_times.insert("windows".to_owned(), now);
        for window in &facts.windows {
            facts
                .field_times
                .insert(format!("windows.{}", window.id), now);
        }
    }
    if facts.extra_usage.is_some()
        || limits.is_some_and(|limits| limits.get("extra_usage").is_some_and(Json::is_null))
    {
        facts.field_times.insert("extra_usage".to_owned(), now);
    }
    if let Some(plan) = body
        .get("subscription_type")
        .and_then(Json::as_str)
        .filter(|plan| !plan.trim().is_empty())
    {
        facts.plan = Some(plan.to_ascii_lowercase());
        facts.field_times.insert("plan".to_owned(), now);
    }
    (!facts.field_times.is_empty()).then_some(facts)
}

fn reset_time(value: Option<&Json>) -> Option<Time> {
    let value = value?;
    value
        .as_str()
        .and_then(|time| time.parse().ok())
        .or_else(|| {
            value
                .as_i64()
                .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
                .map(Time::new)
        })
}

fn percent(value: Option<&Json>) -> Option<f64> {
    value?
        .as_f64()
        .filter(|percent| percent.is_finite() && *percent >= 0.0)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the two vendor shapes supply the same eight independent display fields of a usage window"
)]
fn usage_window(
    id: &str,
    minutes: i64,
    used: f64,
    reset: Option<Time>,
    family: Option<String>,
    active: Option<bool>,
    severity: Option<String>,
    warning: Option<f64>,
) -> UsageWindow {
    UsageWindow {
        id: id.to_owned(),
        label: if minutes == SESSION_MINUTES {
            "5h".to_owned()
        } else {
            "weekly".to_owned()
        },
        duration_minutes: Some(minutes),
        used_percent: used,
        resets_at: reset,
        model_family: family,
        is_active: active,
        reset_not_started: None,
        warning_percent: warning,
        severity,
    }
}

/// Maps the known usage-screen members and leaves additional vendor members uninterpreted.
#[must_use]
pub fn usage_from(value: &Json) -> Option<ProviderFacts> {
    let mut windows = Vec::new();
    if let Some(limits) = value.get("limits").and_then(Json::as_array) {
        for limit in limits {
            let Some(kind) = limit.get("kind").and_then(Json::as_str) else {
                continue;
            };
            let (id, minutes, family) = match kind {
                "session" => ("primary".to_owned(), SESSION_MINUTES, None),
                "weekly_all" => ("secondary".to_owned(), WEEK_MINUTES, None),
                "weekly_scoped" => {
                    let Some(model) = limit.get("scope").and_then(|scope| scope.get("model"))
                    else {
                        continue;
                    };
                    let Some(name) = model
                        .get("display_name")
                        .and_then(Json::as_str)
                        .or_else(|| model.get("id").and_then(Json::as_str))
                    else {
                        continue;
                    };
                    (
                        format!("secondary:{}", name.to_lowercase()),
                        WEEK_MINUTES,
                        Some(name.to_owned()),
                    )
                }
                _ => continue,
            };
            let Some(used) = percent(limit.get("percent")) else {
                continue;
            };
            windows.push(usage_window(
                &id,
                minutes,
                used,
                reset_time(limit.get("resets_at")),
                family,
                limit.get("is_active").and_then(Json::as_bool),
                limit
                    .get("severity")
                    .and_then(Json::as_str)
                    .map(str::to_owned),
                Some(WARNING_PERCENT),
            ));
        }
    }
    for (key, id, minutes, family) in [
        ("five_hour", "primary", SESSION_MINUTES, None),
        ("seven_day", "secondary", WEEK_MINUTES, None),
        (
            "seven_day_opus",
            "secondary:opus",
            WEEK_MINUTES,
            Some("Opus"),
        ),
        (
            "seven_day_sonnet",
            "secondary:sonnet",
            WEEK_MINUTES,
            Some("Sonnet"),
        ),
    ] {
        if windows.iter().any(|window| window.id == id) {
            continue;
        }
        let Some(window) = value.get(key).filter(|window| !window.is_null()) else {
            continue;
        };
        let Some(used) = percent(window.get("utilization")) else {
            continue;
        };
        windows.push(usage_window(
            id,
            minutes,
            used,
            reset_time(window.get("resets_at")),
            family.map(str::to_owned),
            None,
            None,
            Some(WARNING_PERCENT),
        ));
    }
    if let Some(scoped) = value.get("model_scoped").and_then(Json::as_array) {
        for window in scoped {
            let Some(name) = window
                .get("display_name")
                .and_then(Json::as_str)
                .filter(|name| !name.trim().is_empty())
            else {
                continue;
            };
            let Some(used) = percent(window.get("utilization")) else {
                continue;
            };
            let id = format!("secondary:{}", name.to_lowercase());
            if !windows.iter().any(|known| known.id == id) {
                windows.push(usage_window(
                    &id,
                    WEEK_MINUTES,
                    used,
                    reset_time(window.get("resets_at")),
                    Some(name.to_owned()),
                    None,
                    None,
                    Some(WARNING_PERCENT),
                ));
            }
        }
    }
    let extra_usage = value
        .get("extra_usage")
        .filter(|extra| extra.is_object())
        .and_then(|extra| {
            Some(ExtraUsage {
                enabled: extra.get("is_enabled")?.as_bool()?,
                limit: percent(extra.get("monthly_limit")),
                used: percent(extra.get("used_credits")),
                used_percent: percent(extra.get("utilization")),
                unit: extra
                    .get("currency")
                    .and_then(Json::as_str)
                    .map(str::to_owned),
                decimal_places: extra
                    .get("decimal_places")
                    .and_then(Json::as_u64)
                    .and_then(|value| value.try_into().ok()),
                disabled_reason: extra
                    .get("disabled_reason")
                    .and_then(Json::as_str)
                    .map(str::to_owned),
                limit_reached: extra.get("spend_limit_reached").and_then(Json::as_bool),
            })
        });
    if windows.is_empty() && extra_usage.is_none() {
        return None;
    }
    Some(ProviderFacts {
        windows,
        extra_usage,
        ..ProviderFacts::default()
    })
}

/// Maps usage carried by a turn; unlike the usage screen its utilization is a fraction.
#[must_use]
pub fn rate_limit_facts(frame: &Json) -> Option<ProviderFacts> {
    if frame.get("type").and_then(Json::as_str) != Some("rate_limit_event") {
        return None;
    }
    let info = frame.get("rate_limit_info")?;
    let unified = info.get("unifiedWindows").and_then(Json::as_object);
    let binding = info.get("rateLimitType").and_then(Json::as_str);
    let mut windows = Vec::new();
    for (key, id, minutes, family) in [
        ("five_hour", "primary", SESSION_MINUTES, None),
        ("seven_day", "secondary", WEEK_MINUTES, None),
        (
            "seven_day_opus",
            "secondary:opus",
            WEEK_MINUTES,
            Some("Opus"),
        ),
        (
            "seven_day_sonnet",
            "secondary:sonnet",
            WEEK_MINUTES,
            Some("Sonnet"),
        ),
    ] {
        let window = match unified {
            Some(windows) => windows.get(key),
            None if binding == Some(key) => Some(info),
            None => None,
        };
        let Some(window) = window else {
            continue;
        };
        let Some(fraction) = percent(window.get("utilization")) else {
            continue;
        };
        let used = fraction * 100.0;
        if !used.is_finite() {
            continue;
        }
        let active = binding.map(|binding| binding == key);
        windows.push(usage_window(
            id,
            minutes,
            used,
            reset_time(window.get("resetsAt")),
            family.map(str::to_owned),
            active,
            active
                .filter(|active| *active)
                .and_then(|_| info.get("status").and_then(Json::as_str))
                .map(str::to_owned),
            percent(info.get("surpassedThreshold"))
                .map(|mark| mark * 100.0)
                .filter(|mark| *mark <= 100.0)
                .or(Some(WARNING_PERCENT)),
        ));
    }
    let limit_reached =
        (info.get("status").and_then(Json::as_str) == Some("rejected")).then(|| LimitReached {
            kind: binding.unwrap_or("usage_limit").to_owned(),
            resets_at: reset_time(info.get("resetsAt")),
        });
    let extra_usage = info
        .get("overageEnabled")
        .and_then(Json::as_bool)
        .map(|enabled| ExtraUsage {
            enabled,
            limit: None,
            used: None,
            used_percent: None,
            unit: None,
            decimal_places: None,
            disabled_reason: info
                .get("overageDisabledReason")
                .and_then(Json::as_str)
                .map(str::to_owned),
            limit_reached: None,
        });
    let admission_told = matches!(
        info.get("status").and_then(Json::as_str),
        Some("allowed" | "allowed_warning" | "rejected")
    );
    if windows.is_empty() && extra_usage.is_none() && !admission_told {
        return None;
    }
    Some(ProviderFacts {
        windows,
        limit_reached,
        extra_usage,
        ..ProviderFacts::default()
    })
}

/// Learns provider windows and model contexts from a frame before the turn mapper consumes it.
pub async fn observe_frame(frame: &Json, home: Option<&Path>, sink: &EventSink, now: Time) {
    if let Some(mut update) = rate_limit_facts(frame) {
        let mut facts = home.and_then(cached_facts).unwrap_or_default();
        facts.credential_verified = None;
        facts.field_times.remove("credential_verified");
        if !update.windows.is_empty() {
            facts.field_times.insert("windows".to_owned(), now);
        }
        if matches!(
            frame
                .get("rate_limit_info")
                .and_then(|info| info.get("status"))
                .and_then(Json::as_str),
            Some("allowed" | "allowed_warning" | "rejected")
        ) {
            facts.limit_reached = update.limit_reached;
            facts.field_times.insert("limit_reached".to_owned(), now);
        }
        for window in update.windows.drain(..) {
            facts
                .field_times
                .insert(format!("windows.{}", window.id), now);
            if let Some(known) = facts.windows.iter_mut().find(|known| known.id == window.id) {
                *known = window;
            } else {
                facts.windows.push(window);
            }
        }
        if let Some(extra) = update.extra_usage {
            facts
                .field_times
                .insert("extra_usage.enabled".to_owned(), now);
            if extra.disabled_reason.is_some() {
                facts
                    .field_times
                    .insert("extra_usage.disabled_reason".to_owned(), now);
            }
            if let Some(known) = facts.extra_usage.as_mut() {
                known.enabled = extra.enabled;
                if extra.enabled || extra.disabled_reason.is_some() {
                    known.disabled_reason = extra.disabled_reason;
                }
            } else {
                facts.extra_usage = Some(extra);
            }
        }
        if let Some(home) = home {
            keep_facts(home, &facts);
        }
        let _ = sink.emit(AdapterEvent::Facts(facts)).await;
    }
    if frame.get("type").and_then(Json::as_str) == Some("result")
        && let Some(home) = home
        && let Some(usage) = frame.get("modelUsage").and_then(Json::as_object)
    {
        let path = own_file(home, MODEL_WINDOWS_FILE);
        let mut windows: std::collections::BTreeMap<String, i64> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        for (name, usage) in usage {
            if let Some(window) = usage
                .get("contextWindow")
                .and_then(Json::as_i64)
                .filter(|window| *window > 0)
            {
                windows.insert(name.clone(), window);
            }
        }
        let _ = std::fs::create_dir_all(home.join(OWN_DIRECTORY));
        if let Ok(text) = serde_json::to_vec(&windows) {
            let _ = std::fs::write(path, text);
        }
    }
}

/// Supplies a discovered model's missing context, leaving every explicit context untouched.
pub fn add_context_windows(found: &mut [ModelSuggestion], home: Option<&Path>) {
    let windows: std::collections::BTreeMap<String, i64> = home
        .and_then(|home| std::fs::read_to_string(own_file(home, MODEL_WINDOWS_FILE)).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    for model in found {
        if let Some(window) = windows.get(&model.model_name) {
            model.parameters.context_window.get_or_insert(*window);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Models

/// The models the harness lists for its sign-in, from the answer to its first request.
pub async fn models(
    launcher: &dyn Launcher,
    setup: &HarnessSetup,
    token: Option<&str>,
) -> Result<Vec<ModelSuggestion>, AdapterError> {
    let spec = probe_spec(setup, token, "models");
    let (events_tx, mut events) = crate::process::event_channel();
    let running = spawn(launcher, &spec, 0, events_tx)
        .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
    running
        .write(encode_line(&control_request(
            "models",
            json!({ "subtype": "initialize" }),
        )))
        .await;
    let answered = tokio::time::timeout(PROBE_TIMEOUT, async {
        while let Some((_, event)) = events.recv().await {
            match event {
                ProcessEvent::Line(line) => {
                    let Ok(frame) = crate::stream::Frame::parse(&line) else {
                        continue;
                    };
                    if let Some((id, outcome)) = frame.control_outcome()
                        && id == "models"
                    {
                        return Some(outcome);
                    }
                }
                ProcessEvent::Exited(_) => return None,
                ProcessEvent::TooLong | ProcessEvent::Unreadable(_) => {}
            }
        }
        None
    })
    .await;
    running.close_input();
    match answered {
        Ok(Some(crate::stream::ControlOutcome::Success(body))) => Ok(suggestions_from(&body)),
        Ok(Some(crate::stream::ControlOutcome::Error(error))) => {
            Err(AdapterError::HarnessError(error))
        }
        _ => Err(AdapterError::HarnessError(
            "the harness did not list its models".to_owned(),
        )),
    }
}

/// The longest start of a description that is taken for a model's own name and version ("Opus 5.5"); a longer
/// start is a sentence about something else.
const SHOWN_NAME_MAX_CHARS: usize = 40;

/// The alias that is the harness's pointer to one of the other entries (or to a model set by hand).
const DEFAULT_ALIAS: &str = "default";

/// What a person is shown for a listed model: the start of its description when that is its name and
/// version ("Opus 5.5 · Best for ..."), else the short name.
fn shown_name(model: &Json) -> Option<String> {
    let leading = model
        .get("description")
        .and_then(Json::as_str)
        .and_then(|text| text.split_once(" · "))
        .map(|(head, _)| head.trim())
        .filter(|head| {
            !head.is_empty()
                && head.chars().count() <= SHOWN_NAME_MAX_CHARS
                && !head.starts_with("Use ")
        });
    leading
        .or_else(|| model.get("displayName").and_then(Json::as_str))
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// The models in the answer to the first request: the name that is sent is the model's full name where the
/// list gives one, and what a person is shown is the harness's own words for it. The alias `default` points
/// to one of the others, so it comes last and only adds a model nothing else lists.
#[must_use]
pub fn suggestions_from(body: &Json) -> Vec<ModelSuggestion> {
    let listed: Vec<&Json> = body
        .get("models")
        .and_then(Json::as_array)
        .map(|models| models.iter().collect())
        .unwrap_or_default();
    let is_default =
        |model: &&Json| model.get("value").and_then(Json::as_str) == Some(DEFAULT_ALIAS);
    let (defaults, others): (Vec<&Json>, Vec<&Json>) = listed.into_iter().partition(is_default);
    let mut seen = std::collections::BTreeSet::new();
    let mut found = Vec::new();
    for (model, alias) in others
        .into_iter()
        .map(|model| (model, false))
        .chain(defaults.into_iter().map(|model| (model, true)))
    {
        let Some(value) = model.get("value").and_then(Json::as_str) else {
            continue;
        };
        let name = model
            .get("resolvedModel")
            .and_then(Json::as_str)
            .unwrap_or(value);
        if model
            .get("disabled")
            .and_then(Json::as_bool)
            .unwrap_or(false)
            || !seen.insert(name.to_owned())
        {
            continue;
        }
        let efforts: Vec<String> = model
            .get("supportedEffortLevels")
            .and_then(Json::as_array)
            .map(|levels| {
                levels
                    .iter()
                    .filter_map(Json::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let shown = if alias { None } else { shown_name(model) };
        let mut suggestion = ModelSuggestion::named(name, shown.as_deref());
        // A model with a fast mode lists it as on offer; it is enabled by a manager of the provider and by
        // no one else, since it costs more for every turn. The harness's list says nothing of the window.
        let fast = model.get("supportsFastMode").and_then(Json::as_bool) == Some(true);
        suggestion.parameters = ModelParameters {
            efforts: (!efforts.is_empty()).then_some(efforts),
            input_images: Some(true),
            offered: fast.then(|| Offered {
                service_tiers: Some(vec![ServiceTier {
                    id: FAST_TIER.to_owned(),
                    label: "Fast".to_owned(),
                }]),
                ..Offered::default()
            }),
            ..ModelParameters::default()
        };
        found.push(suggestion);
    }
    found
}

// ---------------------------------------------------------------------------------------------------------
// The browser sign-in

/// What a running browser sign-in is.
#[derive(Debug)]
pub struct BrowserLogin {
    /// Writes the code to the harness's input.
    pub input: flow::Sender<String>,
    /// Ends it.
    cancel: watch::Sender<bool>,
    kill: Option<Arc<Notify>>,
}

impl BrowserLogin {
    /// Cancels sign-in without waiting for native output or a full input queue.
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
        if let Some(kill) = &self.kill {
            kill.notify_one();
        }
    }
}

impl Drop for BrowserLogin {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// The address in a line of the sign-in's output, if the line has one.
#[must_use]
pub fn address_in(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let address = line[start..].split_whitespace().next()?;
    Some(address.to_owned())
}

/// The field a person is asked to fill in with the code of the sign-in page.
#[must_use]
pub fn code_field() -> Field {
    let mut field = Field::new("code", "Code", FieldType::Secret);
    field.help = Some("The code the sign-in page shows once you have signed in.".to_owned());
    field
}

/// The state of a sign-in that waits for the code.
#[must_use]
pub fn awaiting_code(address: &str, now: Time) -> LoginState {
    LoginState {
        state: LoginStateKind::AwaitingUser,
        method: Some(METHOD_BROWSER_CODE.to_owned()),
        shows: vec![LoginShow {
            kind: ShowKind::Uri,
            label: "Open this address and sign in".to_owned(),
            value: address.to_owned(),
        }],
        asks: Some(vec![code_field()]),
        expires_at: Some(Time::new(
            now.get() + chrono::Duration::seconds(CODE_WAIT_SECS),
        )),
        error: None,
    }
}

/// Starts the harness's sign-in and reports its progress to `report`. The harness prints an address and
/// waits for a code on its input; the task ends when it does.
pub async fn start_browser_login<F, Report>(
    launcher: Arc<dyn Launcher>,
    setup: &HarnessSetup,
    clock: Arc<dyn Clock>,
    report: F,
) -> Result<BrowserLogin, AdapterError>
where
    F: Fn(LoginState) -> Report + Send + 'static,
    Report: std::future::Future<Output = bool> + Send + 'static,
{
    let spec = auth_spec(setup, None, &["auth", "login"]);
    let (events_tx, mut events) = crate::process::event_channel();
    let mut running = spawn(launcher.as_ref(), &spec, 0, events_tx)
        .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
    let (input_tx, mut input_rx) = flow::channel(|code: &String| code.capacity());
    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    let kill = running.kill_handle();
    if !report(LoginState::starting(METHOD_BROWSER_CODE)).await {
        running.kill();
        return Err(AdapterError::HarnessUnavailable(
            "the host stopped consuming sign-in events".into(),
        ));
    }
    tokio::spawn(async move {
        let deadline = tokio::time::sleep(std::time::Duration::from_secs(
            u64::try_from(CODE_WAIT_SECS).unwrap_or(600) + 60,
        ));
        tokio::pin!(deadline);
        let mut last_line: Option<String> = None;
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Some((_, ProcessEvent::Line(line))) => {
                        if let Some(address) = address_in(&line) && !report(awaiting_code(&address, clock.time())).await {
                            running.kill();
                            return;
                        }
                        last_line = Some(line);
                    }
                    Some((_, ProcessEvent::Exited(exit))) => {
                        if exit.code == Some(0) {
                            report(LoginState::completed(METHOD_BROWSER_CODE)).await;
                        } else {
                            let said = last_line.take().unwrap_or_else(|| exit.describe());
                            report(LoginState::failed(METHOD_BROWSER_CODE, &said)).await;
                        }
                        return;
                    }
                    Some(_) => {}
                    None => return,
                },
                code = input_rx.recv() => {
                    if let Some(code) = code {
                        running.write(format!("{}\n", code.trim())).await;
                    }
                }
                _ = cancel_rx.changed() => {
                    running.kill();
                    report(LoginState::idle()).await;
                    return;
                }
                () = &mut deadline => {
                    running.kill();
                    let mut state = LoginState::idle();
                    state.state = LoginStateKind::Expired;
                    state.method = Some(METHOD_BROWSER_CODE.to_owned());
                    report(state).await;
                    return;
                }
            }
        }
    });
    Ok(BrowserLogin {
        input: input_tx,
        cancel: cancel_tx,
        kill,
    })
}

/// Signs the harness out: its stored login is removed, and so is the token this adapter keeps.
pub async fn logout(launcher: &dyn Launcher, setup: &HarnessSetup) {
    let spec = auth_spec(setup, None, &["auth", "logout"]);
    let _ = run_to_end(launcher, &spec, PROBE_TIMEOUT).await;
    if let Some(home) = &setup.home {
        forget_token(home).await;
        let _ = tokio::fs::remove_file(home.join(LOGIN_FILE)).await;
        let _ = tokio::fs::remove_file(own_file(home, HELD_LOGIN_FILE)).await;
    }
}

// ---------------------------------------------------------------------------------------------------------
// Credentials

fn unusable(why: &str) -> AdapterError {
    AdapterError::CredentialUnusable(why.to_owned())
}

/// The access token of the stored login and when it lapses (milliseconds since the epoch).
fn access_token_of(login: &Json) -> Option<(String, i64)> {
    let oauth = login.get("claudeAiOauth")?;
    Some((
        oauth.get("accessToken")?.as_str()?.to_owned(),
        oauth.get("expiresAt").and_then(Json::as_i64).unwrap_or(0),
    ))
}

/// Produces `form` from what the harness holds in its home. Never more than the form declares: for an
/// access token, not the token that renews it.
pub async fn issue(
    launcher: &dyn Launcher,
    setup: &HarnessSetup,
    clock: &dyn Clock,
    form: &str,
) -> Result<IssuedCredential, AdapterError> {
    let Some(home) = &setup.home else {
        return Err(unusable("the harness has no home of its own to issue from"));
    };
    match form {
        FORM_OAUTH_TOKEN => {
            let stored = stored_token(home)
                .await
                .ok_or_else(|| unusable("no long-lived token is kept; sign in with one first"))?;
            Ok(IssuedCredential {
                form: form.to_owned(),
                expires_at: Some(stored.expires_at()),
                credential: Secret::new(stored.token),
            })
        }
        FORM_ACCESS_TOKEN => {
            let read = || async {
                let text = tokio::fs::read_to_string(home.join(LOGIN_FILE))
                    .await
                    .ok()?;
                access_token_of(&serde_json::from_str::<Json>(&text).ok()?)
            };
            let mut current = read()
                .await
                .ok_or_else(|| unusable("the harness holds no login"))?;
            if current.1 - clock.time().unix_millis() < ACCESS_TOKEN_MARGIN_MILLIS {
                // The harness renews its login when it uses it: asking how it is signed in is use.
                let _ = auth_status(launcher, setup, None).await;
                current = read().await.unwrap_or(current);
            }
            let (token, expires_millis) = current;
            if expires_millis - clock.time().unix_millis() <= 0 {
                return Err(unusable(
                    "the login's access token has lapsed and could not be renewed",
                ));
            }
            Ok(IssuedCredential {
                form: form.to_owned(),
                expires_at: chrono::DateTime::from_timestamp_millis(expires_millis).map(Time::new),
                credential: Secret::new(token),
            })
        }
        FORM_LOGIN => {
            let path = home.join(LOGIN_FILE);
            let text = tokio::fs::read_to_string(&path)
                .await
                .map_err(|_| unusable("the harness holds no login"))?;
            // The login moves: it waits here, unused, until it is returned.
            write_private(&own_file(home, HELD_LOGIN_FILE), text.as_bytes())
                .await
                .map_err(|error| unusable(&error.to_string()))?;
            tokio::fs::remove_file(&path)
                .await
                .map_err(|error| unusable(&error.to_string()))?;
            Ok(IssuedCredential {
                form: form.to_owned(),
                expires_at: None,
                credential: Secret::new(text),
            })
        }
        other => Err(AdapterError::Unsupported(format!(
            "credentials: the form {other:?}"
        ))),
    }
}

/// What installing a credential does to the harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installed {
    /// The harness runs on this token from now on.
    Token(String),
    /// The login is where the harness reads it.
    Login,
}

/// Installs a credential the harness is to run on.
pub async fn install(
    setup: &HarnessSetup,
    credential: &GivenCredential,
) -> Result<Installed, AdapterError> {
    match credential.form.as_str() {
        FORM_ACCESS_TOKEN | FORM_OAUTH_TOKEN => {
            let token = credential.credential.expose();
            if !looks_like_a_token(token) {
                return Err(unusable("the credential is not a token"));
            }
            Ok(Installed::Token(token.trim().to_owned()))
        }
        FORM_LOGIN => {
            let home = setup
                .home
                .as_ref()
                .ok_or_else(|| unusable("the harness has no home of its own to keep a login in"))?;
            write_private(
                &home.join(LOGIN_FILE),
                credential.credential.expose().as_bytes(),
            )
            .await
            .map_err(|error| unusable(&error.to_string()))?;
            Ok(Installed::Login)
        }
        other => Err(unusable(&format!(
            "the form {other:?} is not one the harness runs on"
        ))),
    }
}

/// Stops using a handed-over login and gives back what is current: the file the harness has kept renewed.
pub async fn take_back(setup: &HarnessSetup) -> Result<GivenCredential, AdapterError> {
    let home = setup
        .home
        .as_ref()
        .ok_or_else(|| unusable("the harness has no home of its own"))?;
    let path = home.join(LOGIN_FILE);
    let text = tokio::fs::read_to_string(&path)
        .await
        .map_err(|_| unusable("no handed-over login is held here"))?;
    tokio::fs::remove_file(&path)
        .await
        .map_err(|error| unusable(&error.to_string()))?;
    Ok(GivenCredential {
        form: FORM_LOGIN.to_owned(),
        credential: Secret::new(text),
        expires_at: None,
        upstream: None,
    })
}

/// Takes a returned login back: it is where the harness reads it again, and what waited is let go.
pub async fn accept_back(
    setup: &HarnessSetup,
    credential: &GivenCredential,
) -> Result<(), AdapterError> {
    if credential.form != FORM_LOGIN {
        return Err(unusable("only a login comes home"));
    }
    let home = setup
        .home
        .as_ref()
        .ok_or_else(|| unusable("the harness has no home of its own"))?;
    write_private(
        &home.join(LOGIN_FILE),
        credential.credential.expose().as_bytes(),
    )
    .await
    .map_err(|error| unusable(&error.to_string()))?;
    let _ = tokio::fs::remove_file(own_file(home, HELD_LOGIN_FILE)).await;
    Ok(())
}

/// The method of a sign-in that is a token someone pastes.
#[must_use]
pub fn is_token_method(method: &str) -> bool {
    method == METHOD_SETUP_TOKEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_source_declares_its_existing_limit_report_without_claiming_usage_permission() {
        let facts = fact_kinds();
        assert!(facts.contains(&ProviderFactKind::LimitReached));
        assert!(!facts.contains(&ProviderFactKind::OrdinaryUsageAllowed));
    }
    use crate::config::ProfileConfig;
    use crate::process::DirectLauncher;
    use orch_agent_contract::loopback::Loopback;
    use orch_types::ManualClock;
    use std::collections::BTreeMap;

    fn setup(home: &Path) -> HarnessSetup {
        HarnessSetup {
            program: "claude".into(),
            home: Some(home.to_path_buf()),
            managed_home: true,
            loopback: Loopback::new(41000, Secret::new("t")),
            profile_env: BTreeMap::new(),
            config: ProfileConfig::default(),
            inherited: BTreeMap::new(),
            as_root: false,
        }
    }

    fn at(text: &str) -> Time {
        text.parse().unwrap()
    }

    #[test]
    fn managed_auth_and_discovery_processes_cannot_restore_updates_or_account_sync() {
        let home = tempfile::tempdir().unwrap();
        let mut selected = setup(home.path());
        selected
            .profile_env
            .insert("DISABLE_UPDATES".into(), "0".into());
        selected
            .profile_env
            .insert("DISABLE_TELEMETRY".into(), "0".into());
        selected
            .profile_env
            .insert("ENABLE_CLAUDEAI_MCP_SERVERS".into(), "true".into());
        assert_eq!(auth_environment(&selected, None)["DISABLE_UPDATES"], "1");
        for label in ["usage", "models"] {
            let launch = probe_spec(&selected, None, label);
            assert_eq!(launch.program, selected.program);
            assert_eq!(
                launch.env["CLAUDE_CONFIG_DIR"],
                home.path().display().to_string()
            );
            assert_eq!(launch.env["DISABLE_UPDATES"], "1");
            assert_eq!(launch.env["ENABLE_CLAUDEAI_MCP_SERVERS"], "false");
            let index = launch
                .args
                .iter()
                .position(|arg| arg == "--settings")
                .unwrap();
            let settings: Json = serde_json::from_str(&launch.args[index + 1]).unwrap();
            assert_eq!(settings["disableClaudeAiConnectors"], true);
            assert_eq!(settings["syncClaudeAiSkills"], false);
            assert_eq!(settings["syncClaudeAiPlugins"], false);
            assert_eq!(settings["remoteControlAtStartup"], false);
            assert_eq!(settings["crossSessionInbound"], "refuse");
            assert_eq!(settings["env"]["DISABLE_UPDATES"], "1");
            assert!(launch.args.iter().any(|arg| arg == "--strict-mcp-config"));
        }
        selected.managed_home = false;
        assert_eq!(auth_environment(&selected, None)["DISABLE_UPDATES"], "0");
        assert!(
            !probe_spec(&selected, None, "models")
                .args
                .iter()
                .any(|arg| arg == "--settings")
        );
    }

    #[test]
    fn the_keeper_and_harness_use_the_same_home_environment_and_overrides() {
        let mut selected = setup(Path::new("/unused"));
        selected.home = None;
        selected.managed_home = false;
        selected.inherited.insert("HOME".into(), "/user".into());
        assert_eq!(
            credential_home(&selected).unwrap(),
            Path::new("/user/.claude")
        );
        selected
            .profile_env
            .insert("HOME".into(), "/runtime-user".into());
        assert_eq!(
            credential_home(&selected).unwrap(),
            Path::new("/runtime-user/.claude")
        );
        selected
            .inherited
            .insert("CLAUDE_CONFIG_DIR".into(), "/user-selected-config".into());
        assert_eq!(
            credential_home(&selected).unwrap(),
            Path::new("/user-selected-config")
        );
        selected
            .profile_env
            .insert("CLAUDE_CONFIG_DIR".into(), "/runtime-config".into());
        assert_eq!(
            credential_home(&selected).unwrap(),
            Path::new("/runtime-config")
        );
        selected.home = Some("/chosen-home".into());
        assert_eq!(
            credential_home(&selected).unwrap(),
            Path::new("/chosen-home")
        );
        assert_eq!(
            auth_environment(&selected, None)["CLAUDE_CONFIG_DIR"],
            "/chosen-home"
        );
    }

    #[tokio::test]
    async fn a_pasted_token_is_kept_privately_and_lapses_a_year_later() {
        let home = tempfile::tempdir().unwrap();
        let stored = store_token(
            home.path(),
            "  sk-ant-oat01-abc \n",
            METHOD_SETUP_TOKEN,
            at("2026-10-11T10:00:00Z"),
        )
        .await
        .unwrap();
        assert_eq!(stored.token, "sk-ant-oat01-abc");
        assert_eq!(stored.expires_at(), at("2027-10-11T10:00:00Z"));
        let read = stored_token(home.path()).await.unwrap();
        assert_eq!(read.token, "sk-ant-oat01-abc");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.path().join(".orch/oauth.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        forget_token(home.path()).await;
        assert!(stored_token(home.path()).await.is_none());
    }

    #[test]
    fn only_one_word_can_be_a_token() {
        assert!(looks_like_a_token("sk-ant-oat01-abc_DEF-123"));
        assert!(!looks_like_a_token(""));
        assert!(!looks_like_a_token("two words"));
        assert!(!looks_like_a_token("line\nbreak"));
    }

    #[test]
    fn the_status_the_harness_prints_is_read_for_who_and_which_plan() {
        let none = AuthStatus::from_json(
            &json!({ "loggedIn": false, "authMethod": "none", "apiProvider": "firstParty" }),
        );
        assert_eq!(none, AuthStatus::default());
        let on = AuthStatus::from_json(&json!({
            "loggedIn": true, "authMethod": "claude.ai", "email": "dana@example.com",
            "subscriptionType": "Max", "orgName": "Example"
        }));
        assert!(on.logged_in);
        assert_eq!(on.email.as_deref(), Some("dana@example.com"));
        assert_eq!(on.plan.as_deref(), Some("max"));
        assert_eq!(on.organization.as_deref(), Some("Example"));
    }

    #[test]
    fn the_address_of_the_sign_in_page_is_found_in_what_the_harness_prints() {
        assert_eq!(
            address_in(
                "If the browser didn't open, visit: https://claude.com/cai/oauth/authorize?code=true&state=x"
            ),
            Some("https://claude.com/cai/oauth/authorize?code=true&state=x".into())
        );
        assert_eq!(address_in("Opening browser to sign in…"), None);
        let waiting = awaiting_code("https://example.test/a", at("2026-10-11T10:00:00Z"));
        assert_eq!(waiting.state, LoginStateKind::AwaitingUser);
        assert_eq!(waiting.shows[0].value, "https://example.test/a");
        assert_eq!(waiting.asks.as_ref().unwrap()[0].name, "code");
        assert_eq!(waiting.expires_at, Some(at("2026-10-11T10:10:00Z")));
    }

    #[test]
    fn the_models_the_harness_lists_are_named_by_their_full_name_once_each_in_its_own_words() {
        let suggestions = suggestions_from(&json!({ "models": [
            { "value": "default", "resolvedModel": "claude-opus-5-5", "displayName": "Default (recommended)",
              "description": "Use the default model (currently Opus 5.5) · $4/$20 per Mtok",
              "supportedEffortLevels": ["low", "high"] },
            { "value": "opus", "resolvedModel": "claude-opus-5-5", "displayName": "Opus",
              "description": "Opus 5.5 · Best for everyday, complex tasks · $4/$20 per Mtok",
              "supportedEffortLevels": ["low", "high"] },
            { "value": "haiku", "resolvedModel": "claude-haiku-5-5", "displayName": "Haiku", "supportsEffort": false },
            { "value": "fable", "resolvedModel": "claude-fable-5", "displayName": "Fable", "disabled": true }
        ] }));
        let names: Vec<_> = suggestions.iter().map(|s| s.model_name.as_str()).collect();
        assert_eq!(names, ["claude-opus-5-5", "claude-haiku-5-5"]);
        assert_eq!(
            suggestions[0].display_name, "Opus 5.5",
            "the model's name and version"
        );
        assert_eq!(suggestions[0].parameters.efforts(), ["low", "high"]);
        assert_eq!(
            suggestions[1].display_name, "Haiku",
            "the short name when the description gives no other"
        );
        assert!(suggestions[1].parameters.efforts().is_empty());
    }

    #[test]
    fn the_default_alias_adds_a_model_only_when_nothing_else_lists_it() {
        let suggestions = suggestions_from(&json!({ "models": [
            { "value": "default", "resolvedModel": "my-own-model", "displayName": "Default (recommended)",
              "description": "Use the default model (currently my-own-model) · $1/$2 per Mtok" },
            { "value": "haiku", "resolvedModel": "claude-haiku-5-5", "displayName": "Haiku" }
        ] }));
        let names: Vec<_> = suggestions.iter().map(|s| s.model_name.as_str()).collect();
        assert_eq!(
            names,
            ["claude-haiku-5-5", "my-own-model"],
            "the alias comes last"
        );
        assert_eq!(
            suggestions[1].display_name, "my-own-model",
            "no label of the alias is shown for a model"
        );
    }

    #[tokio::test]
    async fn the_long_lived_token_is_issued_as_it_is_and_nothing_else_is_issued_without_a_login() {
        let home = tempfile::tempdir().unwrap();
        let setup = setup(home.path());
        let clock = ManualClock::new("2026-10-11T10:00:00Z".parse().unwrap());
        assert!(matches!(
            issue(&DirectLauncher, &setup, &clock, FORM_OAUTH_TOKEN).await,
            Err(AdapterError::CredentialUnusable(_))
        ));
        store_token(
            home.path(),
            "sk-ant-oat01-abc",
            METHOD_SETUP_TOKEN,
            at("2026-10-11T10:00:00Z"),
        )
        .await
        .unwrap();
        let issued = issue(&DirectLauncher, &setup, &clock, FORM_OAUTH_TOKEN)
            .await
            .unwrap();
        assert_eq!(issued.credential.expose(), "sk-ant-oat01-abc");
        assert!(matches!(
            issue(&DirectLauncher, &setup, &clock, FORM_ACCESS_TOKEN).await,
            Err(AdapterError::CredentialUnusable(_))
        ));
        assert!(matches!(
            issue(&DirectLauncher, &setup, &clock, "api_key").await,
            Err(AdapterError::Unsupported(_))
        ));
    }

    #[tokio::test]
    async fn an_access_token_is_issued_without_the_token_that_renews_it() {
        let home = tempfile::tempdir().unwrap();
        let setup = setup(home.path());
        let clock = ManualClock::new("2026-10-11T10:00:00Z".parse().unwrap());
        let lapses = at("2026-10-11T12:00:00Z").unix_millis();
        std::fs::write(
            home.path().join(".credentials.json"),
            json!({ "claudeAiOauth": { "accessToken": "sk-ant-oat01-access", "refreshToken": "sk-ant-ort01-SECRET",
                                       "expiresAt": lapses } })
            .to_string(),
        )
        .unwrap();
        let issued = issue(&DirectLauncher, &setup, &clock, FORM_ACCESS_TOKEN)
            .await
            .unwrap();
        assert_eq!(issued.credential.expose(), "sk-ant-oat01-access");
        assert_eq!(issued.expires_at, Some(at("2026-10-11T12:00:00Z")));
        assert!(!serde_json::to_string(&issued).unwrap().contains("SECRET"));
    }

    #[tokio::test]
    async fn the_login_moves_to_the_environment_and_comes_back_as_it_then_is() {
        let provider = tempfile::tempdir().unwrap();
        let environment = tempfile::tempdir().unwrap();
        let provider_setup = setup(provider.path());
        let environment_setup = setup(environment.path());
        let clock = ManualClock::new("2026-10-11T10:00:00Z".parse().unwrap());
        std::fs::write(
            provider.path().join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r"}}"#,
        )
        .unwrap();

        let issued = issue(&DirectLauncher, &provider_setup, &clock, FORM_LOGIN)
            .await
            .unwrap();
        assert!(
            !provider.path().join(".credentials.json").exists(),
            "it is not used here while it is away"
        );
        assert!(provider.path().join(".orch/held-login.json").exists());

        let given = issued.into_given(None);
        assert_eq!(
            install(&environment_setup, &given).await.unwrap(),
            Installed::Login
        );
        assert!(environment.path().join(".credentials.json").exists());
        // The harness there renewed it.
        std::fs::write(
            environment.path().join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"a2","refreshToken":"r2"}}"#,
        )
        .unwrap();
        let back = take_back(&environment_setup).await.unwrap();
        assert!(back.credential.expose().contains("r2"));
        assert!(
            !environment.path().join(".credentials.json").exists(),
            "removed at the return"
        );

        accept_back(&provider_setup, &back).await.unwrap();
        assert!(
            std::fs::read_to_string(provider.path().join(".credentials.json"))
                .unwrap()
                .contains("r2")
        );
        assert!(!provider.path().join(".orch/held-login.json").exists());
    }

    #[tokio::test]
    async fn a_token_given_to_the_harness_must_be_one_word() {
        let home = tempfile::tempdir().unwrap();
        let setup = setup(home.path());
        let given = |text: &str| GivenCredential {
            form: FORM_OAUTH_TOKEN.into(),
            credential: Secret::new(text),
            expires_at: None,
            upstream: None,
        };
        assert_eq!(
            install(&setup, &given("sk-ant-oat01-abc")).await.unwrap(),
            Installed::Token("sk-ant-oat01-abc".into())
        );
        assert!(install(&setup, &given("not a token")).await.is_err());
    }
}
