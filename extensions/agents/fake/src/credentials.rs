//! Shared and transferred test credential lifecycles.
//!
//! The fake has a sign-in of its own, and runs on credentials it is given. This module keeps both.
//!
//! * [`Signin`] runs the three sign-in methods of the descriptor: the code that waits for the person
//!   and ends by itself, the page that asks for a text back, and the credential the person brings.
//!   Every step is told as a `provider.login` event; no secret is in any of them.
//! * [`Credentials`] holds what the sign-in gave and what the harness runs on: the login in the
//!   provider's runtime, from which copies are issued (the form `fake_token` lapses after two minutes,
//!   `api_key` lasts, `fake_login` moves and leaves the login unusable here until it is returned), and
//!   the credential the harness has been given where it runs, which it returns, renews or has
//!   rejected.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_agent_contract::adapter::{AdapterError, AdapterEvent, EventSink};
use orch_agent_contract::credentials::{
    Custody, GivenCredential, IssuedCredential, MIN_ISSUE_INTERVAL_MILLIS,
};
use orch_agent_contract::descriptor::{ArrangementType, ProviderKind};
use orch_agent_contract::facts::{
    CredentialFacts, CredentialYield, LimitReached, PlanCredits, ProviderFacts, UsageWindow,
};
use orch_agent_contract::login::{
    LoginMethodDef, LoginRequest, LoginShow, LoginState, LoginStateKind, LoginValues, LoginYields,
    ShowKind,
};
use orch_plugin_contract::Secret;
use orch_types::{Clock, Field, FieldType, Json, Time};

use crate::descriptor::{
    FORM_KEY, FORM_LOGIN, FORM_TOKEN, IMPORT_TOKEN_LIFETIME_SECS, METHOD_ASK, METHOD_CODE,
    METHOD_IMPORT, TOKEN_LIFETIME_SECS, login_methods,
};
use crate::events::lock;

/// The address the sign-in by a code shows.
pub const DEVICE_ADDRESS: &str = "https://fake.invalid/device";
/// The address the sign-in on a page shows.
pub const AUTHORIZE_ADDRESS: &str = "https://fake.invalid/authorize";
/// How close to its end a copy is when the harness says it will need another: the shortest time
/// between two copies, which a renewal then waits no longer than.
pub const EXPIRY_MARGIN_SECS: i64 = MIN_ISSUE_INTERVAL_MILLIS / 1_000;
/// How long a window of the plan is: five hours.
pub const WINDOW_MINUTES: i64 = 300;
/// How much of the plan's window a turn uses, in percent. A few turns do not use it up.
const PERCENT_PER_TURN: f64 = 5.0;

fn invalid(message: impl Into<String>) -> AdapterError {
    AdapterError::Other {
        code: "invalid".to_owned(),
        message: message.into(),
        retryable: false,
    }
}

fn after(time: Time, seconds: i64) -> Time {
    Time::new(time.get() + chrono::Duration::seconds(seconds))
}

/// What the sign-in gave the provider.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedIn {
    /// The method.
    pub method: String,
    /// A login the harness renews, or a token that lapses.
    pub yields: LoginYields,
    /// When it lapses, where it does.
    pub expires_at: Option<Time>,
    /// Whom it is a sign-in of.
    pub identity: String,
    /// The material of the login. It is never told to anybody except as the form a copy is issued in.
    material: String,
}

/// The credential the harness has been given where it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Held {
    /// What it was given.
    pub given: GivenCredential,
    /// How many times the harness renewed it itself while it held it: only a login that moved.
    pub renewals: u32,
    /// Whether the upstream refused it.
    pub rejected: bool,
}

impl Held {
    /// Whether it has lapsed at `now`.
    #[must_use]
    pub fn lapsed(&self, now: Time) -> bool {
        self.given.expires_at.is_some_and(|end| end <= now)
    }

    /// Whether it lapses within the margin at `now`.
    #[must_use]
    pub fn lapsing(&self, now: Time) -> bool {
        self.given
            .expires_at
            .is_some_and(|end| end.unix_millis() - now.unix_millis() < EXPIRY_MARGIN_SECS * 1_000)
    }
}

/// What the fake holds of credentials.
#[derive(Debug, Default)]
pub struct Credentials {
    /// The sign-in of the provider, in the runtime where the fake is the provider's harness.
    pub signed_in: Option<SignedIn>,
    /// The credential the harness runs on, where it runs.
    pub held: Option<Held>,
    /// Whether the login moved away and has not come back.
    pub moved_away: bool,
    /// The tenant the provider was configured with.
    pub tenant: Option<String>,
    /// How many copies were issued, for the value of the next.
    issued: u64,
    /// When the last copy was issued.
    last_issue: Option<Time>,
    /// Whether the plan is used up.
    pub exhausted: bool,
    /// When the plan is whole again, if it was used up until a moment that was chosen; without it the plan
    /// stays used up, and its windows say they reset five hours from whenever they are asked.
    pub exhausted_until: Option<Time>,
    /// How many turns the harness has run: the plan's windows fill with them.
    pub turns: u32,
}

impl Credentials {
    /// Whether the plan is used up at `now`: it was, and the moment it was told to end has not come.
    #[must_use]
    pub fn used_up(&self, now: Time) -> bool {
        self.exhausted && self.exhausted_until.is_none_or(|until| now < until)
    }

    /// Uses the plan up for `minutes` from `now`, and says when it is whole again.
    pub fn use_up_for(&mut self, now: Time, minutes: i64) -> Time {
        let until = after(now, minutes * 60);
        self.exhausted = true;
        self.exhausted_until = Some(until);
        until
    }

    /// What the fake reports of its provider.
    pub fn facts(&self, now: Time) -> Result<ProviderFacts, AdapterError> {
        let Some(signed) = &self.signed_in else {
            return Err(AdapterError::LoginRequired);
        };
        let used_up = self.used_up(now);
        let used = if used_up {
            100.0
        } else {
            (f64::from(self.turns) * PERCENT_PER_TURN).min(100.0)
        };
        let resets = after(now, WINDOW_MINUTES * 60);
        let lifted = self.exhausted_until.unwrap_or(resets);
        Ok(ProviderFacts {
            plan: Some("pro".to_owned()),
            windows: vec![UsageWindow {
                id: "5h".to_owned(),
                label: "5h".to_owned(),
                duration_minutes: Some(WINDOW_MINUTES),
                used_percent: used,
                resets_at: Some(resets),
                model_family: None,
                is_active: None,
                reset_not_started: None,
                warning_percent: None,
                severity: None,
            }],
            plan_credits: Some(PlanCredits {
                balance: Some("100".to_owned()),
                unit: Some("credits".to_owned()),
                unlimited: false,
            }),
            limit_reached: used_up.then(|| LimitReached {
                kind: "usage_limit".to_owned(),
                resets_at: Some(lifted),
            }),
            identity: Some(signed.identity.clone()),
            credential: Some(CredentialFacts {
                method: Some(signed.method.clone()),
                yields: match signed.yields {
                    LoginYields::Login => CredentialYield::Login,
                    LoginYields::Token => CredentialYield::Token,
                },
                expires_at: signed.expires_at,
                issued_at: None,
            }),
            ..ProviderFacts::default()
        })
    }

    fn require_form(form: &str, accepted: &[String]) -> Result<(), AdapterError> {
        if accepted.iter().any(|name| name == form) {
            Ok(())
        } else {
            Err(AdapterError::CredentialUnusable(format!(
                "the harness does not run on a credential of the form {form:?}"
            )))
        }
    }

    /// Produces a credential in `form` from the login, in the provider's runtime.
    pub fn issue(
        &mut self,
        form: &str,
        issued: &[orch_agent_contract::credentials::CredentialForm],
        now: Time,
    ) -> Result<IssuedCredential, AdapterError> {
        let Some(signed) = self.signed_in.as_ref() else {
            return Err(AdapterError::LoginRequired);
        };
        if self.moved_away {
            return Err(AdapterError::CredentialUnusable(
                "the login is away and has not been returned".to_owned(),
            ));
        }
        let declared = issued
            .iter()
            .find(|declared| declared.form == form)
            .ok_or_else(|| AdapterError::Unsupported(format!("credential form {form}")))?;
        if self
            .last_issue
            .is_some_and(|last| now.unix_millis() - last.unix_millis() < MIN_ISSUE_INTERVAL_MILLIS)
            && declared.custody == Custody::Shared
        {
            return Err(AdapterError::Other {
                code: "too_soon".to_owned(),
                message: "a copy was issued less than 30 seconds ago".to_owned(),
                retryable: true,
            });
        }
        self.issued += 1;
        self.last_issue = Some(now);
        let credential = match form {
            FORM_TOKEN => IssuedCredential {
                form: form.to_owned(),
                credential: Secret::new(format!("fake-token-{}", self.issued)),
                expires_at: Some(after(now, TOKEN_LIFETIME_SECS)),
            },
            FORM_KEY => IssuedCredential {
                form: form.to_owned(),
                credential: Secret::new("fake-key-1"),
                expires_at: None,
            },
            FORM_LOGIN => {
                self.moved_away = true;
                IssuedCredential {
                    form: form.to_owned(),
                    credential: Secret::new(signed.material.clone()),
                    expires_at: None,
                }
            }
            other => {
                return Err(AdapterError::Unsupported(format!(
                    "credential form {other}"
                )));
            }
        };
        Ok(credential)
    }

    /// Takes a credential the harness is to run on, where it runs.
    pub fn set(
        &mut self,
        given: GivenCredential,
        accepted: &[String],
        now: Time,
    ) -> Result<(), AdapterError> {
        Self::require_form(&given.form, accepted)?;
        if given.expires_at.is_some_and(|end| end <= now) {
            return Err(AdapterError::CredentialUnusable(
                "the credential has lapsed already".to_owned(),
            ));
        }
        self.held = Some(Held {
            given,
            renewals: 0,
            rejected: false,
        });
        Ok(())
    }

    /// Stops using a credential that moved here, and gives back its current material.
    pub fn give_back(&mut self) -> Result<GivenCredential, AdapterError> {
        match self.held.take() {
            Some(held) if held.given.form == FORM_LOGIN => {
                let material = if held.renewals == 0 {
                    held.given.credential.expose().to_owned()
                } else {
                    format!("{}~r{}", held.given.credential.expose(), held.renewals)
                };
                Ok(GivenCredential {
                    credential: Secret::new(material),
                    ..held.given
                })
            }
            Some(held) => {
                let form = held.given.form.clone();
                self.held = Some(held);
                Err(AdapterError::Unsupported(format!(
                    "the credential of the form {form} does not move: it is not given back"
                )))
            }
            None => Err(AdapterError::NotFound(
                "the harness holds no credential that moved".to_owned(),
            )),
        }
    }

    /// Takes the login back that moved away, and signs in with what it is now.
    pub fn take_back(&mut self, given: GivenCredential) -> Result<(), AdapterError> {
        if given.form != FORM_LOGIN {
            return Err(AdapterError::CredentialUnusable(format!(
                "a credential of the form {:?} was not away",
                given.form
            )));
        }
        let Some(signed) = self.signed_in.as_mut() else {
            return Err(AdapterError::LoginRequired);
        };
        signed.material = given.credential.expose().to_owned();
        self.moved_away = false;
        Ok(())
    }

    /// A turn started on the credential held: a login that moved renews itself.
    pub fn note_turn_on_held(&mut self) {
        if let Some(held) = self.held.as_mut()
            && held.given.form == FORM_LOGIN
        {
            held.renewals += 1;
        }
    }

    /// The upstream refused what the harness runs on.
    pub fn reject_held(&mut self) {
        if let Some(held) = self.held.as_mut() {
            held.rejected = true;
        }
    }

    /// Whether the harness has what it needs to run a turn on `path`, and if not, why.
    #[must_use]
    pub fn need(
        &self,
        arrangement: ArrangementType,
        now: Time,
    ) -> Option<orch_agent_contract::credentials::CredentialNeed> {
        use orch_agent_contract::credentials::CredentialNeed;
        let carries = matches!(
            arrangement,
            ArrangementType::CredentialShared | ArrangementType::HandedOver
        );
        if !carries {
            return None;
        }
        match &self.held {
            None => Some(CredentialNeed::Missing),
            Some(held) if held.rejected || held.lapsed(now) => Some(CredentialNeed::Rejected),
            Some(held) if held.lapsing(now) => Some(CredentialNeed::Expiring),
            Some(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Signing in

struct Run {
    id: u64,
    state: LoginState,
}

struct SigninState {
    run: Option<Run>,
    started: u64,
    expiry: Option<tokio::task::JoinHandle<()>>,
}

/// Runs the sign-ins of the fake.
#[derive(Clone)]
pub struct Signin {
    sink: EventSink,
    clock: Arc<dyn Clock>,
    methods: Arc<Vec<LoginMethodDef>>,
    code_lifetime: Duration,
    state: Arc<Mutex<SigninState>>,
}

impl std::fmt::Debug for Signin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signin").finish_non_exhaustive()
    }
}

impl Signin {
    /// Sign-ins whose steps are told to `sink`, and whose code is good for `code_lifetime_secs`.
    #[must_use]
    pub fn new(sink: EventSink, clock: Arc<dyn Clock>, code_lifetime_secs: u32) -> Self {
        Self {
            sink,
            clock,
            methods: Arc::new(login_methods()),
            code_lifetime: Duration::from_secs(u64::from(code_lifetime_secs)),
            state: Arc::new(Mutex::new(SigninState {
                run: None,
                started: 0,
                expiry: None,
            })),
        }
    }

    fn code_lifetime_secs(&self) -> i64 {
        i64::try_from(self.code_lifetime.as_secs()).unwrap_or(i64::MAX)
    }

    async fn tell(&self, state: &LoginState) {
        let _ = self.sink.emit(AdapterEvent::Login(state.clone())).await;
    }

    /// Where the sign-in stands.
    #[must_use]
    pub fn standing(&self) -> LoginState {
        lock(&self.state)
            .run
            .as_ref()
            .map_or_else(LoginState::idle, |run| run.state.clone())
    }

    async fn set(&self, id: u64, state: LoginState) {
        {
            let mut guard = lock(&self.state);
            if let Some(run) = guard.run.as_mut()
                && run.id == id
            {
                run.state = state.clone();
            }
        }
        self.tell(&state).await;
    }

    /// Starts a sign-in. Progress is told as events; a method that takes what it needs up front
    /// completes within the call.
    pub async fn start(
        &self,
        request: &LoginRequest,
        credentials: &Mutex<Credentials>,
    ) -> Result<(), AdapterError> {
        let method = self
            .methods
            .iter()
            .find(|method| method.id == request.method)
            .ok_or_else(|| {
                invalid(format!(
                    "{:?} is not a sign-in method of the fake",
                    request.method
                ))
            })?
            .clone();
        let tenant = request
            .config
            .get("tenant")
            .and_then(Json::as_str)
            .map(str::to_owned);
        if let Some(Json::Object(options)) = Some(&request.config)
            && let Some(unknown) = options.keys().find(|key| key.as_str() != "tenant")
        {
            return Err(invalid(format!(
                "{unknown:?} is not an option of the fake provider"
            )));
        }
        let now = self.clock.time();
        let id = {
            let mut guard = lock(&self.state);
            if let Some(task) = guard.expiry.take() {
                task.abort();
            }
            guard.started += 1;
            let id = guard.started;
            guard.run = Some(Run {
                id,
                state: LoginState::starting(&method.id),
            });
            id
        };
        self.tell(&LoginState::starting(&method.id)).await;
        match method.id.as_str() {
            METHOD_CODE => {
                let expires = after(now, self.code_lifetime_secs());
                let mut state = LoginState::starting(METHOD_CODE);
                state.state = LoginStateKind::AwaitingUser;
                state.shows = vec![
                    LoginShow {
                        kind: ShowKind::Uri,
                        label: "Open this address".to_owned(),
                        value: DEVICE_ADDRESS.to_owned(),
                    },
                    LoginShow {
                        kind: ShowKind::Code,
                        label: "Enter this code there".to_owned(),
                        value: format!("FAKE-{id:04}"),
                    },
                ];
                state.expires_at = Some(expires);
                lock(credentials).tenant = tenant;
                self.set(id, state).await;
                self.arm_expiry(id);
                Ok(())
            }
            METHOD_ASK => {
                let mut state = LoginState::starting(METHOD_ASK);
                state.state = LoginStateKind::AwaitingUser;
                state.shows = vec![LoginShow {
                    kind: ShowKind::Uri,
                    label: "Open this address".to_owned(),
                    value: AUTHORIZE_ADDRESS.to_owned(),
                }];
                state.asks = Some(vec![Field::new(
                    "answer",
                    "The text the page shows",
                    FieldType::Text,
                )]);
                state.expires_at = Some(after(now, self.code_lifetime_secs()));
                lock(credentials).tenant = tenant;
                self.set(id, state).await;
                self.arm_expiry(id);
                Ok(())
            }
            METHOD_IMPORT => {
                let outcome = self.import(
                    &method,
                    &request.values,
                    tenant.as_deref(),
                    now,
                    credentials,
                );
                match outcome {
                    Ok(()) => {
                        self.set(id, LoginState::completed(METHOD_IMPORT)).await;
                        Ok(())
                    }
                    Err(error) => {
                        self.set(id, LoginState::failed(METHOD_IMPORT, &error.detail()))
                            .await;
                        Err(error)
                    }
                }
            }
            other => Err(invalid(format!(
                "{other:?} is not a sign-in method of the fake"
            ))),
        }
    }

    fn arm_expiry(&self, id: u64) {
        let this = self.clone();
        let lifetime = self.code_lifetime;
        let task = tokio::spawn(async move {
            tokio::time::sleep(lifetime).await;
            let waiting = {
                let guard = lock(&this.state);
                guard.run.as_ref().and_then(|run| {
                    (run.id == id && run.state.state == LoginStateKind::AwaitingUser)
                        .then(|| run.state.method.clone())
                })
            };
            if let Some(method) = waiting {
                let mut state = LoginState::idle();
                state.state = LoginStateKind::Expired;
                state.method = method;
                this.set(id, state).await;
            }
        });
        lock(&self.state).expiry = Some(task);
    }

    fn import(
        &self,
        method: &LoginMethodDef,
        values: &LoginValues,
        tenant: Option<&str>,
        now: Time,
        credentials: &Mutex<Credentials>,
    ) -> Result<(), AdapterError> {
        if let Err(faults) = method.check_values(values) {
            return Err(invalid(
                faults
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
        let text = values.text("credential_text").filter(|t| !t.is_empty());
        let path = values.text("credential_path").filter(|p| !p.is_empty());
        let material = match (text, path) {
            (Some(text), None) => text.to_owned(),
            (None, Some(path)) => std::fs::read_to_string(path).map_err(|error| {
                AdapterError::NotFound(format!("the credential file {path}: {error}"))
            })?,
            _ => {
                return Err(invalid(
                    "give the credential file's text or its path, one of the two",
                ));
            }
        };
        if material.trim().is_empty() {
            return Err(invalid("the credential is empty"));
        }
        let expires_at = match values.text("expires_at") {
            Some(day) => {
                let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
                    .map_err(|_| invalid("expires_at is a day written YYYY-MM-DD"))?;
                let end = date
                    .and_hms_milli_opt(23, 59, 59, 999)
                    .map(|moment| moment.and_utc())
                    .ok_or_else(|| invalid("expires_at is not a day"))?;
                Time::new(end)
            }
            None => after(now, IMPORT_TOKEN_LIFETIME_SECS),
        };
        let account = values.text("account").unwrap_or("account");
        let mut guard = lock(credentials);
        guard.tenant = tenant.map(str::to_owned);
        guard.signed_in = Some(SignedIn {
            method: METHOD_IMPORT.to_owned(),
            yields: LoginYields::Token,
            expires_at: Some(expires_at),
            identity: identity(account, tenant),
            material,
        });
        guard.moved_away = false;
        Ok(())
    }

    /// Answers what a running sign-in asked for.
    pub async fn input(
        &self,
        values: &LoginValues,
        credentials: &Mutex<Credentials>,
    ) -> Result<(), AdapterError> {
        let (id, state) = {
            let guard = lock(&self.state);
            match guard.run.as_ref() {
                Some(run) if run.state.state == LoginStateKind::AwaitingUser => {
                    (run.id, run.state.clone())
                }
                _ => {
                    return Err(AdapterError::Other {
                        code: "conflict".to_owned(),
                        message: "no sign-in is waiting for the person".to_owned(),
                        retryable: false,
                    });
                }
            }
        };
        let method = state.method.clone().unwrap_or_default();
        if method == METHOD_ASK {
            let answer = values.text("answer").map(str::trim).unwrap_or_default();
            if answer.is_empty() {
                let mut again = state;
                again.error = Some("an answer is needed".to_owned());
                self.set(id, again).await;
                return Ok(());
            }
        }
        if let Some(task) = lock(&self.state).expiry.take() {
            task.abort();
        }
        let tenant = lock(credentials).tenant.clone();
        lock(credentials).signed_in = Some(SignedIn {
            method: method.clone(),
            yields: LoginYields::Login,
            expires_at: None,
            identity: identity("account", tenant.as_deref()),
            material: format!("fake-login-{id}"),
        });
        lock(credentials).moved_away = false;
        self.set(id, LoginState::completed(&method)).await;
        Ok(())
    }

    /// Gives the running sign-in up.
    pub async fn cancel(&self) {
        let id = {
            let mut guard = lock(&self.state);
            if let Some(task) = guard.expiry.take() {
                task.abort();
            }
            guard.run.take().map(|run| run.id)
        };
        if id.is_some() {
            self.tell(&LoginState::idle()).await;
        }
    }

    /// Signs the provider out.
    pub async fn sign_out(&self, credentials: &Mutex<Credentials>) {
        self.cancel().await;
        {
            let mut guard = lock(credentials);
            guard.signed_in = None;
            guard.held = None;
            guard.moved_away = false;
        }
        self.tell(&LoginState::idle()).await;
    }
}

fn identity(account: &str, tenant: Option<&str>) -> String {
    match tenant {
        Some(tenant) => format!("{account}@{tenant}"),
        None => format!("{account}@fake.invalid"),
    }
}

/// The provider kind a credential is for: the fake is a subscription of its own harness.
pub const PROVIDER_KIND: ProviderKind = ProviderKind::Subscription;

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::credentials::CredentialForm;
    use orch_types::ManualClock;
    use std::collections::BTreeMap;

    fn clock() -> Arc<dyn Clock> {
        Arc::new(ManualClock::new(
            "2026-10-11T10:00:00Z"
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap(),
        ))
    }

    fn forms() -> Vec<CredentialForm> {
        crate::descriptor::credential_forms()
    }

    fn signed_in() -> Credentials {
        Credentials {
            signed_in: Some(SignedIn {
                method: METHOD_CODE.into(),
                yields: LoginYields::Login,
                expires_at: None,
                identity: "a@b".into(),
                material: "the-login".into(),
            }),
            ..Credentials::default()
        }
    }

    fn at(offset_secs: i64) -> Time {
        after("2026-10-11T10:00:00.000Z".parse().unwrap(), offset_secs)
    }

    #[test]
    fn a_copy_that_lapses_lasts_two_minutes_and_a_copy_that_lasts_has_no_end() {
        let mut credentials = signed_in();
        let token = credentials.issue(FORM_TOKEN, &forms(), at(0)).unwrap();
        assert_eq!(token.expires_at, Some(at(120)));
        assert!(token.credential.expose().starts_with("fake-token-"));
        let key = credentials.issue(FORM_KEY, &forms(), at(60)).unwrap();
        assert_eq!(key.expires_at, None);
    }

    #[test]
    fn a_second_copy_within_thirty_seconds_is_refused_and_may_be_asked_for_again() {
        let mut credentials = signed_in();
        credentials.issue(FORM_TOKEN, &forms(), at(0)).unwrap();
        let error = credentials.issue(FORM_TOKEN, &forms(), at(10)).unwrap_err();
        assert_eq!(error.code(), "too_soon");
        assert!(error.retryable());
        assert!(credentials.issue(FORM_TOKEN, &forms(), at(30)).is_ok());
    }

    #[test]
    fn nothing_is_issued_without_a_sign_in_or_in_a_form_the_fake_does_not_have() {
        let mut none = Credentials::default();
        assert_eq!(
            none.issue(FORM_TOKEN, &forms(), at(0)).unwrap_err().code(),
            "login_required"
        );
        let mut credentials = signed_in();
        assert_eq!(
            credentials
                .issue("oauth", &forms(), at(0))
                .unwrap_err()
                .code(),
            "unsupported"
        );
    }

    #[test]
    fn a_login_that_moved_is_unusable_here_until_it_comes_back_with_what_it_became() {
        let mut provider = signed_in();
        let moved = provider.issue(FORM_LOGIN, &forms(), at(0)).unwrap();
        assert_eq!(moved.credential.expose(), "the-login");
        assert_eq!(
            provider
                .issue(FORM_TOKEN, &forms(), at(60))
                .unwrap_err()
                .code(),
            "credential_unusable",
            "no copy is made of a login that is away"
        );
        let accepted: Vec<String> = [FORM_TOKEN, FORM_KEY, FORM_LOGIN]
            .map(str::to_owned)
            .to_vec();
        let mut environment = Credentials::default();
        environment
            .set(moved.into_given(None), &accepted, at(1))
            .unwrap();
        environment.note_turn_on_held();
        environment.note_turn_on_held();
        let back = environment.give_back().unwrap();
        assert_eq!(
            back.credential.expose(),
            "the-login~r2",
            "the current material comes back"
        );
        assert!(environment.held.is_none());
        provider.take_back(back).unwrap();
        assert!(!provider.moved_away);
        let again = provider.issue(FORM_LOGIN, &forms(), at(90)).unwrap();
        assert_eq!(again.credential.expose(), "the-login~r2");
    }

    #[test]
    fn a_credential_that_does_not_move_is_not_given_back() {
        let accepted: Vec<String> = [FORM_TOKEN].map(str::to_owned).to_vec();
        let mut environment = Credentials::default();
        let given = GivenCredential {
            form: FORM_TOKEN.into(),
            credential: Secret::new("t"),
            expires_at: Some(at(120)),
            upstream: None,
        };
        environment.set(given, &accepted, at(0)).unwrap();
        assert_eq!(environment.give_back().unwrap_err().code(), "unsupported");
        assert!(environment.held.is_some(), "it is still held");
    }

    #[test]
    fn a_harness_asks_for_another_copy_when_it_has_none_when_it_lapsed_and_when_it_is_about_to() {
        use orch_agent_contract::credentials::CredentialNeed;
        let accepted: Vec<String> = [FORM_TOKEN].map(str::to_owned).to_vec();
        let mut environment = Credentials::default();
        assert_eq!(
            environment.need(ArrangementType::CredentialShared, at(0)),
            Some(CredentialNeed::Missing)
        );
        assert_eq!(environment.need(ArrangementType::Together, at(0)), None);
        let given = GivenCredential {
            form: FORM_TOKEN.into(),
            credential: Secret::new("t"),
            expires_at: Some(at(120)),
            upstream: None,
        };
        environment.set(given, &accepted, at(0)).unwrap();
        assert_eq!(
            environment.need(ArrangementType::CredentialShared, at(60)),
            None
        );
        assert_eq!(
            environment.need(ArrangementType::CredentialShared, at(100)),
            Some(CredentialNeed::Expiring)
        );
        assert_eq!(
            environment.need(ArrangementType::CredentialShared, at(121)),
            Some(CredentialNeed::Rejected)
        );
        environment
            .set(
                GivenCredential {
                    form: FORM_TOKEN.into(),
                    credential: Secret::new("t2"),
                    expires_at: Some(at(400)),
                    upstream: None,
                },
                &accepted,
                at(130),
            )
            .unwrap();
        environment.reject_held();
        assert_eq!(
            environment.need(ArrangementType::CredentialShared, at(131)),
            Some(CredentialNeed::Rejected)
        );
    }

    #[test]
    fn a_credential_that_has_lapsed_or_is_of_a_form_the_harness_does_not_run_on_is_refused() {
        let accepted: Vec<String> = [FORM_TOKEN].map(str::to_owned).to_vec();
        let mut environment = Credentials::default();
        let lapsed = GivenCredential {
            form: FORM_TOKEN.into(),
            credential: Secret::new("t"),
            expires_at: Some(at(-1)),
            upstream: None,
        };
        assert_eq!(
            environment
                .set(lapsed, &accepted, at(0))
                .unwrap_err()
                .code(),
            "credential_unusable"
        );
        let other = GivenCredential {
            form: FORM_LOGIN.into(),
            credential: Secret::new("t"),
            expires_at: None,
            upstream: None,
        };
        assert_eq!(
            environment.set(other, &accepted, at(0)).unwrap_err().code(),
            "credential_unusable"
        );
    }

    #[test]
    fn the_facts_of_a_provider_need_a_sign_in_and_fill_with_turns() {
        let mut credentials = Credentials::default();
        assert_eq!(
            credentials.facts(at(0)).unwrap_err().code(),
            "login_required"
        );
        credentials.signed_in = signed_in().signed_in;
        credentials.turns = 3;
        let facts = credentials.facts(at(0)).unwrap();
        assert!((facts.windows[0].used_percent - 15.0).abs() < f64::EPSILON);
        assert!(!facts.is_exhausted());
        credentials.exhausted = true;
        assert!(credentials.facts(at(0)).unwrap().is_exhausted());
    }

    #[test]
    fn a_plan_used_up_until_a_moment_says_that_moment_and_is_whole_again_after_it() {
        let mut credentials = Credentials {
            signed_in: signed_in().signed_in,
            ..Credentials::default()
        };
        let until = credentials.use_up_for(at(0), 90);
        assert_eq!(until, at(90 * 60));
        // The moment stands: it does not slide with the time of the question, as five hours from now does.
        for asked in [0, 60, 90 * 60 - 1] {
            let facts = credentials.facts(at(asked)).unwrap();
            let limit = facts.limit_reached.unwrap();
            assert_eq!(limit.resets_at, Some(until), "asked at {asked}");
        }
        assert!(!credentials.facts(at(90 * 60)).unwrap().is_exhausted());
        assert!(!credentials.used_up(at(90 * 60 + 1)));
    }

    // The sign-in itself, with a sink that is read.

    fn sink() -> (EventSink, orch_agent_contract::adapter::EventStream) {
        EventSink::channel()
    }

    fn states(stream: &mut orch_agent_contract::adapter::EventStream) -> Vec<LoginState> {
        let mut states = Vec::new();
        while let Ok(AdapterEvent::Login(state)) = stream.try_recv() {
            states.push(state);
        }
        states
    }

    #[tokio::test(start_paused = true)]
    async fn a_code_is_shown_and_ends_by_itself_when_nobody_answers() {
        let (sink, mut stream) = sink();
        let signin = Signin::new(sink, clock(), 600);
        let credentials = Mutex::new(Credentials::default());
        signin
            .start(
                &LoginRequest {
                    method: METHOD_CODE.into(),
                    values: LoginValues::default(),
                    config: Json::Null,
                },
                &credentials,
            )
            .await
            .unwrap();
        let seen = states(&mut stream);
        assert_eq!(seen[0].state, LoginStateKind::Starting);
        assert_eq!(seen[1].state, LoginStateKind::AwaitingUser);
        assert_eq!(seen[1].shows[0].value, DEVICE_ADDRESS);
        assert_eq!(seen[1].shows[1].kind, ShowKind::Code);
        assert!(seen[1].asks.is_none());
        tokio::time::sleep(Duration::from_secs(601)).await;
        let later = states(&mut stream);
        assert_eq!(later.len(), 1);
        assert_eq!(later[0].state, LoginStateKind::Expired);
        assert!(credentials.lock().unwrap().signed_in.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_code_completes_when_the_test_says_so_and_does_not_expire_after() {
        let (sink, mut stream) = sink();
        let signin = Signin::new(sink, clock(), 600);
        let credentials = Mutex::new(Credentials::default());
        let request = LoginRequest {
            method: METHOD_CODE.into(),
            values: LoginValues::default(),
            config: json_object(&[("tenant", "acme")]),
        };
        signin.start(&request, &credentials).await.unwrap();
        signin
            .input(&LoginValues::default(), &credentials)
            .await
            .unwrap();
        let seen = states(&mut stream);
        assert_eq!(seen.last().unwrap().state, LoginStateKind::Completed);
        let signed = credentials.lock().unwrap().signed_in.clone().unwrap();
        assert_eq!(signed.identity, "account@acme");
        tokio::time::sleep(Duration::from_secs(700)).await;
        assert!(states(&mut stream).is_empty(), "nothing more is told");
    }

    fn json_object(pairs: &[(&str, &str)]) -> Json {
        Json::Object(
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), Json::from(*v)))
                .collect(),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn a_page_asks_for_a_text_and_is_not_satisfied_with_none() {
        let (sink, mut stream) = sink();
        let signin = Signin::new(sink, clock(), 600);
        let credentials = Mutex::new(Credentials::default());
        signin
            .start(
                &LoginRequest {
                    method: METHOD_ASK.into(),
                    values: LoginValues::default(),
                    config: Json::Null,
                },
                &credentials,
            )
            .await
            .unwrap();
        let seen = states(&mut stream);
        let asking = seen.last().unwrap();
        assert_eq!(asking.asks.as_ref().unwrap()[0].name, "answer");
        signin
            .input(&LoginValues::default(), &credentials)
            .await
            .unwrap();
        let refused = states(&mut stream);
        assert_eq!(refused[0].state, LoginStateKind::AwaitingUser);
        assert_eq!(refused[0].error.as_deref(), Some("an answer is needed"));
        let answer = LoginValues(BTreeMap::from([("answer".to_owned(), Json::from("K7-ZQ"))]));
        signin.input(&answer, &credentials).await.unwrap();
        assert_eq!(states(&mut stream)[0].state, LoginStateKind::Completed);
        assert!(credentials.lock().unwrap().signed_in.is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn a_sign_in_that_is_cancelled_leaves_nothing_and_a_second_answer_has_no_one_to_hear_it()
    {
        let (sink, mut stream) = sink();
        let signin = Signin::new(sink, clock(), 600);
        let credentials = Mutex::new(Credentials::default());
        signin
            .start(
                &LoginRequest {
                    method: METHOD_CODE.into(),
                    values: LoginValues::default(),
                    config: Json::Null,
                },
                &credentials,
            )
            .await
            .unwrap();
        let _ = states(&mut stream);
        signin.cancel().await;
        assert_eq!(states(&mut stream)[0].state, LoginStateKind::Idle);
        let error = signin
            .input(&LoginValues::default(), &credentials)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "conflict");
        tokio::time::sleep(Duration::from_secs(700)).await;
        assert!(
            states(&mut stream).is_empty(),
            "a cancelled code does not expire"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_credential_that_is_brought_needs_its_text_or_its_path_and_one_of_them_only() {
        let (sink, mut stream) = sink();
        let signin = Signin::new(sink, clock(), 600);
        let credentials = Mutex::new(Credentials::default());
        let request = |values: &[(&str, &str)]| LoginRequest {
            method: METHOD_IMPORT.into(),
            values: LoginValues(
                values
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), Json::from(*v)))
                    .collect(),
            ),
            config: Json::Null,
        };
        let neither = signin.start(&request(&[]), &credentials).await.unwrap_err();
        assert_eq!(neither.code(), "invalid");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("credentials");
        std::fs::write(&file, "the-secret").unwrap();
        let both = signin
            .start(
                &request(&[
                    ("credential_text", "x"),
                    ("credential_path", file.to_str().unwrap()),
                ]),
                &credentials,
            )
            .await
            .unwrap_err();
        assert_eq!(both.code(), "invalid");
        assert!(credentials.lock().unwrap().signed_in.is_none());
        let _ = states(&mut stream);
        signin
            .start(
                &request(&[
                    ("credential_path", file.to_str().unwrap()),
                    ("expires_at", "2026-12-24"),
                ]),
                &credentials,
            )
            .await
            .unwrap();
        let signed = credentials.lock().unwrap().signed_in.clone().unwrap();
        assert_eq!(signed.yields, LoginYields::Token);
        assert_eq!(
            signed.expires_at.unwrap().to_string(),
            "2026-12-24T23:59:59.999Z",
            "the day the person gave is the token's end"
        );
        let seen = states(&mut stream);
        assert_eq!(seen.last().unwrap().state, LoginStateKind::Completed);
        assert!(
            !format!("{seen:?}").contains("the-secret"),
            "no secret is in any state"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_token_without_a_day_ends_after_its_declared_lifetime() {
        let (sink, _stream) = sink();
        let signin = Signin::new(sink, clock(), 600);
        let credentials = Mutex::new(Credentials::default());
        signin
            .start(
                &LoginRequest {
                    method: METHOD_IMPORT.into(),
                    values: LoginValues(BTreeMap::from([(
                        "credential_text".to_owned(),
                        Json::from("t"),
                    )])),
                    config: Json::Null,
                },
                &credentials,
            )
            .await
            .unwrap();
        let signed = credentials.lock().unwrap().signed_in.clone().unwrap();
        assert_eq!(signed.expires_at, Some(at(600)));
    }

    #[tokio::test(start_paused = true)]
    async fn an_option_the_fake_provider_does_not_have_is_refused() {
        let (sink, _stream) = sink();
        let signin = Signin::new(sink, clock(), 600);
        let credentials = Mutex::new(Credentials::default());
        let error = signin
            .start(
                &LoginRequest {
                    method: METHOD_CODE.into(),
                    values: LoginValues::default(),
                    config: json_object(&[("region", "mars")]),
                },
                &credentials,
            )
            .await
            .unwrap_err();
        assert!(error.detail().contains("region"), "{error}");
    }
}
