//! Sign-in methods as declarations (fields, kind, what each yields; an import takes a credential as text or as the path of a file inside the runtime), the state a running sign-in reports, and a provider's own options.
//!
//! A subscription provider is signed in by one of the ways its adapter declares ([`LoginMethodDef`]): an
//! id, a title, the fields it asks for up front, and what it yields. The dialog that adds a provider
//! renders the declarations and knows no harness. A method that leads the person through something
//! (**interactive**) reports what to show and what to ask as it goes ([`LoginState`]); one that takes a
//! credential the person already has (**import**) is done in one request. An import method takes the
//! credential's text, and where its adapter says so also the path of a file inside the provider's
//! runtime: those are two fields of the method, each optional, and the adapter answers when neither or both
//! are given.
//!
//! What a person gives for a secret field is sealed as soon as it is received. In this crate it is a
//! [`LoginValues`], whose `Debug` shows the names and never the values.

use std::collections::BTreeMap;
use std::fmt;

use orch_types::fields::{FieldError, validate};
use orch_types::{Field, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Whether the harness leads the sign-in or the person brings what they have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginKind {
    /// The harness leads: it shows things, and may ask for something back.
    Interactive,
    /// The person brings a credential they already have.
    Import,
}

/// What a sign-in method gives the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginYields {
    /// A sign-in that the harness renews by itself.
    Login,
    /// A token that is used as it is until it lapses or is revoked.
    Token,
}

/// One way a provider of a harness can be signed in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LoginMethodDef {
    /// The adapter's own name for it.
    pub id: String,
    /// What the dialog calls it.
    pub title: String,
    /// A sentence or two on what happens.
    pub summary: String,
    /// Whether the harness leads or the person brings something.
    pub kind: LoginKind,
    /// What the person gives up front.
    pub fields: Vec<Field>,
    /// What it gives the provider.
    pub yields: LoginYields,
    /// For a token: its usual life in seconds, where the vendor states one.
    pub lifetime_secs: Option<i64>,
    /// What has to be so before it can be used, in the vendor's words; `null` for nothing.
    pub requires: Option<String>,
    /// Whether it is the one a dialog opens on; at most one method of an adapter is.
    pub recommended: bool,
}

/// What is wrong with an adapter's list of sign-in methods.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MethodFault {
    /// A method has no id.
    #[error("a sign-in method needs an id")]
    NoId,
    /// Two methods have one id.
    #[error("sign-in method {0:?} is declared twice")]
    DuplicateId(String),
    /// Two or more methods are recommended.
    #[error("at most one sign-in method is recommended")]
    TwoRecommended,
    /// A method that yields a login states the life of a token.
    #[error("sign-in method {0:?} yields a login and states a lifetime")]
    LifetimeOfALogin(String),
    /// A lifetime is a positive number of seconds.
    #[error("sign-in method {0:?} has a lifetime that is not positive")]
    LifetimeNotPositive(String),
    /// Two fields of a method have one name.
    #[error("sign-in method {method:?} asks for {field:?} twice")]
    DuplicateField {
        /// The method.
        method: String,
        /// The field.
        field: String,
    },
    /// A method that imports asks for nothing to import.
    #[error("sign-in method {0:?} imports a credential and asks for none")]
    ImportAsksForNothing(String),
}

/// Checks the sign-in methods an adapter declares: each has an id, ids are not repeated, at most one
/// is recommended, only a token has a lifetime, no method names a field twice, and an import asks for
/// something. Every fault is returned.
pub fn check_methods(methods: &[LoginMethodDef]) -> Result<(), Vec<MethodFault>> {
    let mut faults = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for method in methods {
        if method.id.is_empty() {
            faults.push(MethodFault::NoId);
        } else if seen.contains(&method.id.as_str()) {
            faults.push(MethodFault::DuplicateId(method.id.clone()));
        } else {
            seen.push(&method.id);
        }
        if method.yields == LoginYields::Login && method.lifetime_secs.is_some() {
            faults.push(MethodFault::LifetimeOfALogin(method.id.clone()));
        }
        if method.lifetime_secs.is_some_and(|secs| secs <= 0) {
            faults.push(MethodFault::LifetimeNotPositive(method.id.clone()));
        }
        if method.kind == LoginKind::Import && method.fields.is_empty() {
            faults.push(MethodFault::ImportAsksForNothing(method.id.clone()));
        }
        for (position, field) in method.fields.iter().enumerate() {
            if method.fields[..position]
                .iter()
                .any(|other| other.name == field.name)
            {
                faults.push(MethodFault::DuplicateField {
                    method: method.id.clone(),
                    field: field.name.clone(),
                });
            }
        }
    }
    if methods.iter().filter(|method| method.recommended).count() > 1 {
        faults.push(MethodFault::TwoRecommended);
    }
    if faults.is_empty() {
        Ok(())
    } else {
        Err(faults)
    }
}

impl LoginMethodDef {
    /// Checks what a person gave against the fields this method asks for: every required field is
    /// answered, every answer fits its field, nothing else is given. Every fault is returned.
    pub fn check_values(&self, values: &LoginValues) -> Result<(), Vec<FieldError>> {
        validate(
            &self.fields,
            &Json::Object(values.0.clone().into_iter().collect()),
        )
    }

    /// The names of the fields whose values are secrets: sealed as soon as they are received.
    #[must_use]
    pub fn secret_fields(&self) -> Vec<&str> {
        self.fields
            .iter()
            .filter(|field| field.kind.is_secret())
            .map(|field| field.name.as_str())
            .collect()
    }
}

/// The method of `methods` that a dialog opens on: the recommended one, else the first.
#[must_use]
pub fn default_method(methods: &[LoginMethodDef]) -> Option<&LoginMethodDef> {
    methods
        .iter()
        .find(|method| method.recommended)
        .or_else(|| methods.first())
}

/// Whether a provider has any option to configure: its schema is more than the empty object. An empty
/// schema means there is nothing to configure, and the form is not shown.
#[must_use]
pub fn has_provider_options(options_schema: &Json) -> bool {
    match options_schema {
        Json::Object(schema) => !schema.is_empty(),
        Json::Null => false,
        _ => true,
    }
}

/// What a person gave for the fields of a sign-in method, by field name. Secret values are in it as the
/// text they are: it is the adapter that is trusted with them. It shows no value when it is printed.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct LoginValues(pub BTreeMap<String, Json>);

impl LoginValues {
    /// The value given for `field`.
    #[must_use]
    pub fn get(&self, field: &str) -> Option<&Json> {
        self.0.get(field)
    }

    /// The text given for `field`, when it is a text.
    #[must_use]
    pub fn text(&self, field: &str) -> Option<&str> {
        self.0.get(field).and_then(Json::as_str)
    }
}

impl fmt::Debug for LoginValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

/// The request to start a sign-in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LoginRequest {
    /// The id of one of the adapter's methods.
    pub method: String,
    /// What the person gave up front, as the method's fields ask.
    #[serde(default)]
    pub values: LoginValues,
    /// The provider's own options (`providers.config`), as its adapter's schema allows.
    #[serde(default)]
    pub config: Json,
}

/// Where a sign-in stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginStateKind {
    /// None is running.
    Idle,
    /// One was asked for and has not shown anything yet.
    Starting,
    /// It waits for the person: to do something somewhere, or to answer.
    AwaitingUser,
    /// It succeeded.
    Completed,
    /// It failed, for the reason given beside it.
    Failed,
    /// It was not finished in time.
    Expired,
}

impl LoginStateKind {
    /// Whether it has ended, one way or the other.
    #[must_use]
    pub const fn is_final(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Expired)
    }

    /// Whether a person may give an answer to it now.
    #[must_use]
    pub const fn takes_input(self) -> bool {
        matches!(self, Self::AwaitingUser)
    }
}

/// What kind of thing a running sign-in shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShowKind {
    /// An address to open.
    Uri,
    /// A code to enter there.
    Code,
    /// A command to run.
    Command,
    /// A sentence.
    Text,
}

/// One thing a running sign-in shows the person.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct LoginShow {
    /// What kind of thing it is.
    pub kind: ShowKind,
    /// What it is called.
    pub label: String,
    /// The thing itself.
    pub value: String,
}

/// What a sign-in reports as it goes: what to show, and what to ask back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LoginState {
    /// Where it stands.
    pub state: LoginStateKind,
    /// The method running, or the one last used.
    pub method: Option<String>,
    /// What the person has to see now: an address to open, a code to enter there, a command to run, a
    /// sentence.
    pub shows: Vec<LoginShow>,
    /// What the person has to give back now, if anything.
    pub asks: Option<Vec<Field>>,
    /// When it lapses if nothing happens.
    pub expires_at: Option<Time>,
    /// Why it failed.
    pub error: Option<String>,
}

impl LoginState {
    /// No sign-in is running.
    #[must_use]
    pub fn idle() -> Self {
        Self {
            state: LoginStateKind::Idle,
            method: None,
            shows: Vec::new(),
            asks: None,
            expires_at: None,
            error: None,
        }
    }

    /// A sign-in by `method` that was asked for and has shown nothing yet.
    #[must_use]
    pub fn starting(method: &str) -> Self {
        Self {
            state: LoginStateKind::Starting,
            method: Some(method.to_owned()),
            ..Self::idle()
        }
    }

    /// A sign-in that has ended well.
    #[must_use]
    pub fn completed(method: &str) -> Self {
        Self {
            state: LoginStateKind::Completed,
            method: Some(method.to_owned()),
            ..Self::idle()
        }
    }

    /// A sign-in that failed for `error`.
    #[must_use]
    pub fn failed(method: &str, error: &str) -> Self {
        Self {
            state: LoginStateKind::Failed,
            method: Some(method.to_owned()),
            error: Some(error.to_owned()),
            ..Self::idle()
        }
    }

    /// Whether this sign-in has run out of time at `now`: it waits for the person, and its end has passed.
    #[must_use]
    pub fn is_overdue(&self, now: Time) -> bool {
        self.state == LoginStateKind::AwaitingUser && self.expires_at.is_some_and(|end| end <= now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::FieldType;
    use serde_json::json;

    fn import(id: &str) -> LoginMethodDef {
        LoginMethodDef {
            id: id.into(),
            title: "Bring a token".into(),
            summary: "Paste the token.".into(),
            kind: LoginKind::Import,
            fields: vec![Field::new("token", "Token", FieldType::Secret)],
            yields: LoginYields::Token,
            lifetime_secs: Some(31_536_000),
            requires: None,
            recommended: false,
        }
    }

    #[test]
    fn a_method_reads_as_the_spec_writes_it() {
        let method: LoginMethodDef = serde_json::from_value(json!({
            "id": "device_code", "title": "Sign in with a code",
            "summary": "Orch shows a short code. You enter it on the vendor's sign-in page.",
            "kind": "interactive", "fields": [], "yields": "login", "lifetime_secs": null,
            "requires": "Device-code sign-in has to be switched on in the person's security settings.",
            "recommended": true
        }))
        .unwrap();
        assert_eq!(method.kind, LoginKind::Interactive);
        assert!(method.recommended);
        assert_eq!(check_methods(&[method]), Ok(()));
    }

    #[test]
    fn a_list_of_methods_is_refused_for_each_thing_that_is_wrong_with_it() {
        let mut two = import("setup_token");
        two.recommended = true;
        let mut other = import("setup_token");
        other.recommended = true;
        other.yields = LoginYields::Login;
        other.fields = vec![];
        let faults = check_methods(&[two, other]).unwrap_err();
        assert!(faults.contains(&MethodFault::DuplicateId("setup_token".into())));
        assert!(faults.contains(&MethodFault::TwoRecommended));
        assert!(faults.contains(&MethodFault::LifetimeOfALogin("setup_token".into())));
        assert!(faults.contains(&MethodFault::ImportAsksForNothing("setup_token".into())));
    }

    #[test]
    fn what_a_person_gave_is_checked_against_the_fields_of_the_method_and_never_printed() {
        let method = import("setup_token");
        let values: LoginValues =
            serde_json::from_value(json!({ "token": "sk-ant-oat01-secret" })).unwrap();
        assert_eq!(method.check_values(&values), Ok(()));
        assert!(!format!("{values:?}").contains("sk-ant"));
        assert_eq!(format!("{values:?}"), "[\"token\"]");
        let faults = method.check_values(&LoginValues::default()).unwrap_err();
        assert_eq!(faults[0].path, "token");
        assert_eq!(method.secret_fields(), ["token"]);
    }

    #[test]
    fn a_dialog_opens_on_the_recommended_method_and_otherwise_on_the_first() {
        let mut first = import("a");
        let mut second = import("b");
        assert_eq!(
            default_method(&[first.clone(), second.clone()]).map(|m| m.id.as_str()),
            Some("a")
        );
        second.recommended = true;
        first.recommended = false;
        assert_eq!(
            default_method(&[first, second]).map(|m| m.id.as_str()),
            Some("b")
        );
        assert!(default_method(&[]).is_none());
    }

    #[test]
    fn an_empty_options_schema_means_there_is_nothing_to_configure() {
        assert!(!has_provider_options(&json!({})));
        assert!(has_provider_options(
            &json!({ "type": "object", "properties": { "workspace_id": {} } })
        ));
    }

    #[test]
    fn a_running_sign_in_reports_what_to_show_and_what_to_ask() {
        let state: LoginState = serde_json::from_value(json!({
            "state": "awaiting_user", "method": "device_code",
            "shows": [ { "kind": "uri", "label": "Open", "value": "https://vendor.example/device" },
                       { "kind": "code", "label": "Code", "value": "ABCD-1234" } ],
            "asks": null, "expires_at": "2026-10-11T10:15:00.000Z", "error": null
        }))
        .unwrap();
        assert!(state.state.takes_input());
        assert!(!state.state.is_final());
        assert!(!state.is_overdue("2026-10-11T10:14:59.999Z".parse().unwrap()));
        assert!(state.is_overdue("2026-10-11T10:15:00.000Z".parse().unwrap()));
        assert!(LoginState::completed("device_code").state.is_final());
        assert_eq!(LoginState::failed("m", "no").error.as_deref(), Some("no"));
    }
}
