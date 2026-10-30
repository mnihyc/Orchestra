//! Verified identity claims and linking outcomes.
//!
//! What a provider has vouched for ([`VerifiedIdentity`]), under which names it says so ([`ClaimNames`]), and
//! what the system makes of it: the person is let in, a user is made, the identity is linked to the user who is
//! signed in, or the sign-in is refused with one of eight codes ([`decide`], [`SignInError`]).
//!
//! These rules do not depend on the protocol, so every kind of provider shares them:
//!
//! - **An address counts only where the provider asserts it verified** ([`VerifiedIdentity::trusted_email`]).
//!   One that is not asserted finds nobody, opens nothing and is not kept as the new user's address.
//! - **The list of domains is looked at on every callback**, for an identity that is linked already as for a new
//!   one, so that narrowing the list takes effect for everybody ([`email_domain_allowed`]).
//! - **A person is found by the provider and the subject**, never by the address. When an existing user has the
//!   same verified address and the identity is not linked, nobody is joined by it: the sign-in is refused with
//!   `link_required`, and the person links the identity from their profile while signed in the old way.
//! - **A name for a new user** is made from the claims ([`NewUser`]), valid as a username whatever the provider
//!   sent, and made unique by a numeric suffix.

use orch_types::{Id, Json, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::provider::ProviderSettings;

/// The claim that names the person, unless the provider is set to use another.
pub const DEFAULT_CLAIM_SUBJECT: &str = "sub";
/// The claim that holds the address, unless the provider is set to use another.
pub const DEFAULT_CLAIM_EMAIL: &str = "email";
/// The claim that holds the name to show, unless the provider is set to use another.
pub const DEFAULT_CLAIM_NAME: &str = "name";
/// The claim that holds the name to sign in with, unless the provider is set to use another.
pub const DEFAULT_CLAIM_USERNAME: &str = "preferred_username";
/// The claim in which a provider says it has confirmed the address; its name is the standard one and cannot be
/// set, because a provider that says it under another name is not one that can be trusted with the address.
pub const EMAIL_VERIFIED_CLAIM: &str = "email_verified";
/// The claim that says when the provider last proved who the person is, in whole seconds since the epoch.
pub const AUTH_TIME_CLAIM: &str = "auth_time";

/// The most characters of a subject: the limit the OpenID Connect core gives the `sub` claim.
pub const SUBJECT_MAX_CHARS: u32 = 255;
/// The fewest characters of a username: what the `users` table accepts.
pub const USERNAME_MIN_CHARS: u32 = 3;
/// The most characters of a username: what the `users` table accepts.
pub const USERNAME_MAX_CHARS: u32 = 32;
/// The most characters of a display name: what the `users` table accepts.
pub const DISPLAY_NAME_MAX_CHARS: u32 = 64;
/// The username made when the claims give nothing usable, before the numeric suffix.
pub const USERNAME_FALLBACK: &str = "user";
/// How many names are tried before a username is given up on: far more than any group of people can share one
/// handle, and a bound on the questions a caller that asks the database each time will put.
pub const USERNAME_ATTEMPTS: u32 = 1000;

fn default_subject() -> String {
    DEFAULT_CLAIM_SUBJECT.to_owned()
}

fn default_email() -> String {
    DEFAULT_CLAIM_EMAIL.to_owned()
}

fn default_name() -> String {
    DEFAULT_CLAIM_NAME.to_owned()
}

fn default_username() -> String {
    DEFAULT_CLAIM_USERNAME.to_owned()
}

/// The names under which a provider is set to say who somebody is. They are written with the provider's other
/// members as `claim_subject`, `claim_email`, `claim_name` and `claim_username`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClaimNames {
    /// The claim that names the person at the provider; the provider and this value together are the identity.
    #[serde(rename = "claim_subject", default = "default_subject")]
    pub subject: String,
    /// The claim that holds the address.
    #[serde(rename = "claim_email", default = "default_email")]
    pub email: String,
    /// The claim that holds the name to show.
    #[serde(rename = "claim_name", default = "default_name")]
    pub name: String,
    /// The claim that holds the name to sign in with.
    #[serde(rename = "claim_username", default = "default_username")]
    pub username: String,
}

impl Default for ClaimNames {
    fn default() -> Self {
        Self {
            subject: default_subject(),
            email: default_email(),
            name: default_name(),
            username: default_username(),
        }
    }
}

/// Why the claims name nobody.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClaimsError {
    /// The claim that names the person is not there, is empty, or is neither a text nor a whole number.
    #[error("the claim `{claim}` that names the person is missing")]
    NoSubject {
        /// The claim looked for.
        claim: String,
    },
    /// The subject is longer than a subject may be.
    #[error(
        "the claim `{claim}` that names the person is longer than {SUBJECT_MAX_CHARS} characters"
    )]
    SubjectTooLong {
        /// The claim looked at.
        claim: String,
    },
}

/// What a provider has vouched for about one person, once the kind has checked everything that makes it the
/// provider's word: the signature on it, who issued it and for whom, that it has not ended, and that it answers
/// the request this sign-in made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerifiedIdentity {
    /// The person at the provider; with the provider, the key of the identity.
    pub subject: String,
    /// The address as the provider gave it.
    pub email: Option<String>,
    /// Whether the provider asserts that it has confirmed the address.
    pub email_verified: bool,
    /// The name to show.
    pub name: Option<String>,
    /// The name to sign in with.
    pub username: Option<String>,
    /// When the provider last proved who the person is, where it says (`auth_time`, whole seconds): what a
    /// fresh proof for one sensitive act is compared with.
    #[serde(default)]
    pub auth_time: Option<Time>,
}

impl VerifiedIdentity {
    /// Reads the claims of a provider under the names it is set to use.
    ///
    /// The subject must be a text that is not empty and not longer than [`SUBJECT_MAX_CHARS`], or a whole number
    /// (some providers number their people, and the name of the claim is the provider's setting); the others are
    /// read when they are texts that are not empty. `email_verified` is read when it is `true` or the text
    /// `"true"` in any case, because some providers send it as a text.
    pub fn from_claims(names: &ClaimNames, claims: &Json) -> Result<Self, ClaimsError> {
        let no_subject = || ClaimsError::NoSubject {
            claim: names.subject.clone(),
        };
        let subject = match claims.get(&names.subject).ok_or_else(no_subject)? {
            Json::String(text) if !text.trim().is_empty() => text.clone(),
            Json::Number(number) if number.is_i64() || number.is_u64() => number.to_string(),
            _ => return Err(no_subject()),
        };
        if count_chars(&subject) > SUBJECT_MAX_CHARS {
            return Err(ClaimsError::SubjectTooLong {
                claim: names.subject.clone(),
            });
        }
        let email_verified = match claims.get(EMAIL_VERIFIED_CLAIM) {
            Some(Json::Bool(value)) => *value,
            Some(Json::String(text)) => text.trim().eq_ignore_ascii_case("true"),
            _ => false,
        };
        // The time is whole seconds since the epoch, as a number; anything else says nothing of it.
        let auth_time = claims
            .get(AUTH_TIME_CLAIM)
            .and_then(Json::as_i64)
            .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
            .map(Time::new);
        Ok(Self {
            subject,
            email: text_claim(claims, &names.email),
            email_verified,
            name: text_claim(claims, &names.name),
            username: text_claim(claims, &names.username),
            auth_time,
        })
    }

    /// The address, where the provider asserts that it has confirmed it; otherwise nothing. Compare it without
    /// regard to case: the table of users keeps one address once, however it is written.
    #[must_use]
    pub fn trusted_email(&self) -> Option<&str> {
        if !self.email_verified {
            return None;
        }
        self.email
            .as_deref()
            .map(str::trim)
            .filter(|email| !email.is_empty())
    }
}

fn text_claim(claims: &Json, name: &str) -> Option<String> {
    match claims.get(name) {
        Some(Json::String(text)) if !text.trim().is_empty() => Some(text.trim().to_owned()),
        _ => None,
    }
}

fn count_chars(text: &str) -> u32 {
    u32::try_from(text.chars().count()).unwrap_or(u32::MAX)
}

/// The domain of an address: what follows its last `@`, if anything does.
#[must_use]
pub fn email_domain(email: &str) -> Option<&str> {
    let (_, domain) = email.trim().rsplit_once('@')?;
    let domain = domain.trim();
    (!domain.is_empty()).then_some(domain)
}

/// Whether an address may sign in through a provider whose list of domains is `allowed`. An empty list admits
/// any address, and any sign-in without one. A list that is not empty admits an address whose domain is one of
/// the listed, compared without regard to case; a subdomain is another domain, and a missing address is not
/// admitted. Pass the address only where it is [trusted](VerifiedIdentity::trusted_email).
#[must_use]
pub fn email_domain_allowed(allowed: &[String], email: Option<&str>) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let Some(domain) = email.and_then(email_domain) else {
        return false;
    };
    let domain = domain.to_lowercase();
    allowed
        .iter()
        .any(|entry| entry.trim().trim_start_matches('@').to_lowercase() == domain)
}

/// Makes a handle of one piece of text: lower case, and only letters `a` to `z`, digits, `_` and `-`. A run of
/// other characters (a dash among them) becomes one dash between what is kept, and none at the ends; the handle
/// starts with a letter or a digit and is cut to the longest username. Letters that are not in `a` to `z` are
/// not carried over, so a handle made of such a name may be short or empty.
fn handle_of(text: &str) -> String {
    let mut handle = String::new();
    let mut gap = false;
    for letter in text.to_lowercase().chars() {
        if letter.is_ascii_lowercase() || letter.is_ascii_digit() || letter == '_' {
            if gap && !handle.is_empty() {
                handle.push('-');
            }
            gap = false;
            handle.push(letter);
        } else {
            gap = true;
        }
    }
    let handle = handle.trim_start_matches('_');
    let cut: String = handle.chars().take(USERNAME_MAX_CHARS as usize).collect();
    cut.trim_end_matches('-').to_owned()
}

/// The base of the username of somebody who signs in for the first time: the first of the username claim, the
/// part of the address before its `@`, and the name, that makes a handle of at least [`USERNAME_MIN_CHARS`]
/// characters; [`USERNAME_FALLBACK`] when none does. Many providers send no username claim at all.
#[must_use]
pub fn username_base(identity: &VerifiedIdentity) -> String {
    let local_part = identity
        .email
        .as_deref()
        .and_then(|email| email.trim().split('@').next());
    let from_username = identity.username.as_deref().map(|name| {
        // A username that is an address, as some providers send, is the part before the `@`.
        name.trim().split('@').next().unwrap_or("")
    });
    [from_username, local_part, identity.name.as_deref()]
        .into_iter()
        .flatten()
        .map(handle_of)
        .find(|handle| count_chars(handle) >= USERNAME_MIN_CHARS)
        .unwrap_or_else(|| USERNAME_FALLBACK.to_owned())
}

/// The first of `base`, `base2`, `base3` and so on that `is_taken` does not refuse; `None` after
/// [`USERNAME_ATTEMPTS`] names. `base` is cut where the suffix needs room, so that every name fits. A name is
/// taken when any user ever had it (a username is never reused) or a root org has it as its slug (the two share
/// one namespace); the caller answers for both, best from one query for the names that begin with the base.
pub fn unique_username(base: &str, mut is_taken: impl FnMut(&str) -> bool) -> Option<String> {
    if !is_taken(base) {
        return Some(base.to_owned());
    }
    for number in 2..=USERNAME_ATTEMPTS {
        let suffix = number.to_string();
        let room = (USERNAME_MAX_CHARS as usize).saturating_sub(suffix.len());
        let stem: String = base.chars().take(room).collect();
        let candidate = format!("{stem}{suffix}");
        if !is_taken(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// The name to show for a new user: the name claim, trimmed and cut to [`DISPLAY_NAME_MAX_CHARS`] characters,
/// and the username where the provider sent none.
#[must_use]
pub fn display_name(name: Option<&str>, username: &str) -> String {
    let shown: String = name
        .map(str::trim)
        .unwrap_or_default()
        .chars()
        .take(DISPLAY_NAME_MAX_CHARS as usize)
        .collect();
    let shown = shown.trim_end();
    if shown.is_empty() {
        username.to_owned()
    } else {
        shown.to_owned()
    }
}

/// What a user made from an identity starts with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NewUser {
    /// The username: unique, and valid for the `users` table.
    pub username: String,
    /// The name to show.
    pub display_name: String,
    /// The address, only where the provider asserted it verified, and then already confirmed.
    pub email: Option<String>,
}

impl NewUser {
    /// Makes the user of an identity. `is_taken` is asked as [`unique_username`] asks. `None` when every name
    /// is taken, in which case the caller refuses the sign-in.
    pub fn from_identity(
        identity: &VerifiedIdentity,
        is_taken: impl FnMut(&str) -> bool,
    ) -> Option<Self> {
        let username = unique_username(&username_base(identity), is_taken)?;
        Some(Self {
            display_name: display_name(identity.name.as_deref(), &username),
            email: identity.trusted_email().map(str::to_owned),
            username,
        })
    }
}

/// Who may register: the value of the system setting `registration.mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RegistrationMode {
    /// Nobody: users are made by a system admin.
    Closed,
    /// Only somebody who holds an invitation that is still good.
    Invite,
    /// Anybody who reaches the system.
    Open,
}

/// How the callback ends, apart from errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// A person signs in, with an invitation or without one.
    SignIn {
        /// Whether the sign-in carries an invitation that is still good.
        good_invitation: bool,
    },
    /// A person who is signed in links the identity to their user.
    Link {
        /// The signed-in user.
        user: Id,
    },
}

/// A user an identity is linked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Account {
    /// The user.
    pub user: Id,
    /// Whether a system admin has suspended them.
    pub suspended: bool,
}

/// What the database says of an identity before it is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Lookup {
    /// The user linked to this provider and this subject, if any.
    pub linked: Option<Account>,
    /// A user whose confirmed address is the identity's trusted address, if there is one; asked for only where
    /// the identity has a trusted address.
    pub same_email: Option<Id>,
}

/// What the callback does for an identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Sign this user in, and note the time of it on the link.
    SignIn {
        /// The user the identity is linked to.
        user: Id,
    },
    /// Make a user from the identity ([`NewUser`]), with an org of their own, link it, put them in the org and
    /// role the provider names where it names them, and redeem the invitation where there is one.
    Register,
    /// Link the identity to this user, who is signed in; where it is linked to them already, nothing changes.
    Link {
        /// The signed-in user.
        user: Id,
    },
}

/// The eight ways a sign-in through a provider ends without one; each is the word the sign-in page is sent
/// with, as `/login?error=<code>`, and puts into a sentence.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema, thiserror::Error,
)]
#[serde(rename_all = "snake_case")]
pub enum SignInError {
    /// The provider says the person did not agree, or it refused them (`access_denied`); or the link was refused
    /// because the identity is another user's.
    #[error("the sign-in was denied")]
    Denied,
    /// The sign-in the callback answers is not there, was used already, began more than ten minutes ago, or
    /// was made for another provider.
    #[error("the sign-in took too long, or never began here")]
    StateExpired,
    /// The provider has a list of domains and the address is not on it, or is not one the provider confirmed.
    #[error("the address is not one this provider lets in")]
    DomainNotAllowed,
    /// A new user cannot be made this way: the provider does not register, or the system does not take people.
    #[error("nobody can register through this provider")]
    RegistrationClosed,
    /// The system takes people by invitation and none that is good came with this sign-in.
    #[error("registration needs an invitation that is still good")]
    InviteRequired,
    /// A user has the same confirmed address: they sign in the old way and link the identity from their profile.
    #[error("a user has this address already; link the identity from the profile")]
    LinkRequired,
    /// The provider is switched off, or the user is suspended.
    #[error("this way of signing in is switched off for the person")]
    Disabled,
    /// The provider failed, answered wrongly, or is not set up as it should be.
    #[error("the identity provider failed")]
    IdpError,
}

impl SignInError {
    /// All eight, in the order the interface lists them.
    pub const ALL: [Self; 8] = [
        Self::Denied,
        Self::StateExpired,
        Self::DomainNotAllowed,
        Self::RegistrationClosed,
        Self::InviteRequired,
        Self::LinkRequired,
        Self::Disabled,
        Self::IdpError,
    ];

    /// The code, as it is written in the address of the sign-in page.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::StateExpired => "state_expired",
            Self::DomainNotAllowed => "domain_not_allowed",
            Self::RegistrationClosed => "registration_closed",
            Self::InviteRequired => "invite_required",
            Self::LinkRequired => "link_required",
            Self::Disabled => "disabled",
            Self::IdpError => "idp_error",
        }
    }

    /// The error with this code.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|error| error.code() == code)
    }

    /// Where the browser is sent for this error: the sign-in page, with the code.
    #[must_use]
    pub fn login_path(self) -> String {
        format!("/login?error={}", self.code())
    }
}

/// Decides what the callback does for an identity.
///
/// The order is the order of the checks, and the first that fails is the answer:
///
/// 1. A provider that is switched off lets nobody through: `disabled`.
/// 2. When the provider has a list of domains, the trusted address must be on it: `domain_not_allowed`.
/// 3. An identity that is linked already:
///    - on a sign-in, its user is signed in, or `disabled` where they are suspended;
///    - on a link, it is the signed-in user's own, which changes nothing, or another user's, which is `denied`.
/// 4. An identity that is not linked:
///    - on a link, it is linked to the signed-in user, whatever the address;
///    - on a sign-in, an existing user with the same confirmed address is `link_required` (whether or not the
///      provider registers); then a provider that does not register, or a system that takes nobody, is
///      `registration_closed`; then a system that takes people by invitation, with none that is good, is
///      `invite_required`; otherwise a user is made.
pub fn decide(
    settings: &ProviderSettings,
    mode: RegistrationMode,
    flow: Flow,
    identity: &VerifiedIdentity,
    lookup: &Lookup,
) -> Result<Decision, SignInError> {
    if !settings.enabled {
        return Err(SignInError::Disabled);
    }
    if !email_domain_allowed(&settings.allowed_email_domains, identity.trusted_email()) {
        return Err(SignInError::DomainNotAllowed);
    }
    match (lookup.linked, flow) {
        (Some(account), Flow::SignIn { .. }) if account.suspended => Err(SignInError::Disabled),
        (Some(account), Flow::SignIn { .. }) => Ok(Decision::SignIn { user: account.user }),
        (Some(account), Flow::Link { user }) if account.user == user => Ok(Decision::Link { user }),
        (Some(_), Flow::Link { .. }) => Err(SignInError::Denied),
        (None, Flow::Link { user }) => Ok(Decision::Link { user }),
        (None, Flow::SignIn { good_invitation }) => {
            if identity.trusted_email().is_some() && lookup.same_email.is_some() {
                return Err(SignInError::LinkRequired);
            }
            if !settings.auto_register || mode == RegistrationMode::Closed {
                return Err(SignInError::RegistrationClosed);
            }
            if mode == RegistrationMode::Invite && !good_invitation {
                return Err(SignInError::InviteRequired);
            }
            Ok(Decision::Register)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::BTreeSet;

    fn settings() -> ProviderSettings {
        serde_json::from_value(json!({
            "key": "acme", "name": "Acme", "issuer": "https://login.acme.example",
            "client_id": "orch", "auto_register": true
        }))
        .unwrap()
    }

    fn dana() -> VerifiedIdentity {
        VerifiedIdentity {
            subject: "248289761001".into(),
            email: Some("dana@acme.example".into()),
            email_verified: true,
            name: Some("Dana Whit".into()),
            username: Some("dana.whit".into()),
            auth_time: None,
        }
    }

    fn user(n: u128) -> Id {
        Id::from_u128(n)
    }

    #[test]
    fn claims_are_read_under_the_names_the_provider_is_set_to_use() {
        let claims = json!({
            "sub": "248289761001", "email": " Dana@Acme.example ", "email_verified": true,
            "name": "Dana Whit", "preferred_username": "dana.whit", "locale": "en"
        });
        let identity = VerifiedIdentity::from_claims(&ClaimNames::default(), &claims).unwrap();
        assert_eq!(
            identity,
            VerifiedIdentity {
                subject: "248289761001".into(),
                email: Some("Dana@Acme.example".into()),
                email_verified: true,
                name: Some("Dana Whit".into()),
                username: Some("dana.whit".into()),
                auth_time: None,
            }
        );
        let names: ClaimNames = serde_json::from_value(json!({
            "claim_subject": "oid", "claim_email": "mail", "claim_name": "displayName", "claim_username": "upn"
        }))
        .unwrap();
        let claims = json!({
            "oid": 7, "mail": "d@acme.example", "email_verified": "True", "displayName": "D", "upn": "d@acme.example",
            "sub": "ignored"
        });
        let identity = VerifiedIdentity::from_claims(&names, &claims).unwrap();
        assert_eq!(identity.subject, "7");
        assert!(identity.email_verified);
        assert_eq!(identity.username.as_deref(), Some("d@acme.example"));
    }

    #[test]
    fn the_names_of_the_claims_are_written_with_the_members_of_the_provider_and_default_when_missing()
     {
        let names: ClaimNames = serde_json::from_value(json!({})).unwrap();
        assert_eq!(names, ClaimNames::default());
        assert_eq!(
            serde_json::to_value(&names).unwrap(),
            json!({
                "claim_subject": "sub", "claim_email": "email", "claim_name": "name",
                "claim_username": "preferred_username"
            })
        );
    }

    #[test]
    fn a_subject_that_is_missing_empty_of_the_wrong_kind_or_too_long_names_nobody() {
        let names = ClaimNames::default();
        for claims in [
            json!({}),
            json!({ "sub": "" }),
            json!({ "sub": "  " }),
            json!({ "sub": null }),
            json!({ "sub": true }),
            json!({ "sub": 1.5 }),
            json!({ "sub": ["a"] }),
        ] {
            assert_eq!(
                VerifiedIdentity::from_claims(&names, &claims),
                Err(ClaimsError::NoSubject {
                    claim: "sub".into()
                }),
                "{claims}"
            );
        }
        let longest = "a".repeat(255);
        assert!(VerifiedIdentity::from_claims(&names, &json!({ "sub": longest })).is_ok());
        let too_long = "a".repeat(256);
        assert_eq!(
            VerifiedIdentity::from_claims(&names, &json!({ "sub": too_long })),
            Err(ClaimsError::SubjectTooLong {
                claim: "sub".into()
            })
        );
    }

    #[test]
    fn an_address_is_trusted_only_where_the_provider_asserts_it_verified() {
        let read =
            |claims: Json| VerifiedIdentity::from_claims(&ClaimNames::default(), &claims).unwrap();
        let verified = read(json!({ "sub": "1", "email": "a@b.example", "email_verified": true }));
        assert_eq!(verified.trusted_email(), Some("a@b.example"));
        let as_text = read(json!({ "sub": "1", "email": "a@b.example", "email_verified": "true" }));
        assert_eq!(as_text.trusted_email(), Some("a@b.example"));
        for claims in [
            json!({ "sub": "1", "email": "a@b.example" }),
            json!({ "sub": "1", "email": "a@b.example", "email_verified": false }),
            json!({ "sub": "1", "email": "a@b.example", "email_verified": "yes" }),
            json!({ "sub": "1", "email": "a@b.example", "email_verified": 1 }),
            json!({ "sub": "1", "email": "", "email_verified": true }),
            json!({ "sub": "1", "email_verified": true }),
        ] {
            assert_eq!(read(claims.clone()).trusted_email(), None, "{claims}");
        }
    }

    #[test]
    fn a_list_of_domains_admits_an_address_of_a_listed_domain_in_any_case_and_nothing_else() {
        let acme = ["acme.example".to_owned(), "@Corp.Example".to_owned()];
        assert!(
            email_domain_allowed(&[], None),
            "an empty list admits anything"
        );
        assert!(email_domain_allowed(&[], Some("a@b.example")));
        assert!(email_domain_allowed(&acme, Some("dana@acme.example")));
        assert!(email_domain_allowed(&acme, Some("Dana@ACME.Example")));
        assert!(
            email_domain_allowed(&acme, Some("x@corp.example")),
            "an entry may begin with an @"
        );
        assert!(
            !email_domain_allowed(&acme, Some("dana@mail.acme.example")),
            "a subdomain is another domain"
        );
        assert!(!email_domain_allowed(&acme, Some("dana@evilacme.example")));
        assert!(
            !email_domain_allowed(&acme, Some("acme.example")),
            "no @, no domain"
        );
        assert!(!email_domain_allowed(&acme, Some("dana@")));
        assert!(
            !email_domain_allowed(&acme, None),
            "a list admits nobody without an address"
        );
        assert_eq!(
            email_domain("a@b@c.example"),
            Some("c.example"),
            "the last @ divides"
        );
    }

    #[test]
    fn a_handle_is_lower_case_letters_digits_underscore_and_dashes_between_them() {
        for (text, handle) in [
            ("dana", "dana"),
            ("Dana.Whit", "dana-whit"),
            ("dana  whit", "dana-whit"),
            ("--dana--", "dana"),
            ("_dana_", "dana_"),
            ("a-_b", "a-_b"),
            ("a--b", "a-b"),
            ("D4n4", "d4n4"),
            ("josé", "jos"),
            ("张三", ""),
            ("", ""),
        ] {
            assert_eq!(handle_of(text), handle, "{text:?}");
        }
        let long = "abcdefghij".repeat(5);
        assert_eq!(handle_of(&long).len(), 32);
        let cut_at_a_dash = format!("{}-{}", "a".repeat(31), "b".repeat(10));
        assert_eq!(
            handle_of(&cut_at_a_dash),
            "a".repeat(31),
            "no dash is left at the end"
        );
    }

    #[test]
    fn the_username_is_made_from_the_username_claim_then_the_address_then_the_name_then_user() {
        let mut identity = dana();
        assert_eq!(username_base(&identity), "dana-whit");
        identity.username = Some("dwhit@acme.example".into());
        assert_eq!(
            username_base(&identity),
            "dwhit",
            "an address as a username is cut at the @"
        );
        identity.username = None;
        assert_eq!(
            username_base(&identity),
            "dana",
            "the part of the address before the @"
        );
        identity.email = None;
        assert_eq!(username_base(&identity), "dana-whit", "the name");
        identity.name = Some("张三".into());
        assert_eq!(username_base(&identity), "user");
        identity.username = Some("ab".into());
        identity.name = Some("Dana".into());
        assert_eq!(
            username_base(&identity),
            "dana",
            "a handle shorter than three is passed over"
        );
    }

    #[test]
    fn a_username_that_is_taken_is_made_unique_by_a_number() {
        let taken: BTreeSet<&str> = ["dana", "dana2", "dana3", "acme"].into_iter().collect();
        let is_taken = |name: &str| taken.contains(name);
        assert_eq!(unique_username("sam", is_taken).as_deref(), Some("sam"));
        assert_eq!(unique_username("dana", is_taken).as_deref(), Some("dana4"));
        assert_eq!(
            unique_username("acme", is_taken).as_deref(),
            Some("acme2"),
            "an org's slug counts"
        );
    }

    #[test]
    fn the_number_takes_room_from_a_long_name_so_that_every_name_fits() {
        let long = "a".repeat(32);
        let mut taken: BTreeSet<String> = BTreeSet::new();
        taken.insert(long.clone());
        let second = unique_username(&long, |name| taken.contains(name)).unwrap();
        assert_eq!(second, format!("{}2", "a".repeat(31)));
        taken.insert(second);
        for number in 3..10 {
            taken.insert(format!("{}{number}", "a".repeat(31)));
        }
        let tenth = unique_username(&long, |name| taken.contains(name)).unwrap();
        assert_eq!(tenth, format!("{}10", "a".repeat(30)));
        assert_eq!(tenth.len(), 32);
    }

    #[test]
    fn a_name_nobody_can_have_after_the_attempts_is_given_up_on() {
        let mut asked = 0;
        assert_eq!(
            unique_username("dana", |_| {
                asked += 1;
                true
            }),
            None
        );
        assert_eq!(
            asked, USERNAME_ATTEMPTS,
            "the base, then the numbers 2 up to the limit"
        );
    }

    #[test]
    fn a_username_made_here_is_valid_for_the_table_of_users() {
        let valid = |name: &str| {
            let mut chars = name.chars();
            (3..=32).contains(&name.chars().count())
                && chars
                    .next()
                    .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && chars
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        };
        let long = "Z".repeat(80);
        for text in [
            "Dana.Whit",
            "_x_",
            "a",
            "ab",
            "张三",
            "x@y",
            "-1-",
            long.as_str(),
        ] {
            let identity = VerifiedIdentity {
                subject: "1".into(),
                email: None,
                email_verified: false,
                name: None,
                username: Some(text.into()),
                auth_time: None,
            };
            let user = NewUser::from_identity(&identity, |_| false).unwrap();
            assert!(
                valid(&user.username),
                "{text:?} came to {:?}",
                user.username
            );
        }
    }

    #[test]
    fn a_new_user_has_a_confirmed_address_only_where_the_provider_asserted_it() {
        let user = NewUser::from_identity(&dana(), |_| false).unwrap();
        assert_eq!(
            user,
            NewUser {
                username: "dana-whit".into(),
                display_name: "Dana Whit".into(),
                email: Some("dana@acme.example".into()),
            }
        );
        let mut unconfirmed = dana();
        unconfirmed.email_verified = false;
        assert_eq!(
            NewUser::from_identity(&unconfirmed, |_| false)
                .unwrap()
                .email,
            None
        );
    }

    #[test]
    fn the_display_name_is_the_name_claim_cut_to_the_limit_or_else_the_username() {
        assert_eq!(display_name(Some("  Dana Whit  "), "dana"), "Dana Whit");
        assert_eq!(display_name(None, "dana"), "dana");
        assert_eq!(display_name(Some("   "), "dana"), "dana");
        let long = "名".repeat(80);
        assert_eq!(display_name(Some(&long), "dana").chars().count(), 64);
    }

    #[test]
    fn the_eight_codes_are_those_of_the_callback_and_each_has_its_login_address() {
        let codes: Vec<&str> = SignInError::ALL.iter().map(|error| error.code()).collect();
        assert_eq!(
            codes,
            [
                "denied",
                "state_expired",
                "domain_not_allowed",
                "registration_closed",
                "invite_required",
                "link_required",
                "disabled",
                "idp_error"
            ]
        );
        for error in SignInError::ALL {
            assert_eq!(SignInError::from_code(error.code()), Some(error));
            assert_eq!(serde_json::to_value(error).unwrap(), json!(error.code()));
            assert_eq!(error.login_path(), format!("/login?error={}", error.code()));
        }
        assert_eq!(SignInError::from_code("registration.closed"), None);
        assert_eq!(
            SignInError::LinkRequired.login_path(),
            "/login?error=link_required"
        );
    }

    fn decide_signing_in(
        settings: &ProviderSettings,
        mode: RegistrationMode,
        good_invitation: bool,
        lookup: &Lookup,
    ) -> Result<Decision, SignInError> {
        decide(
            settings,
            mode,
            Flow::SignIn { good_invitation },
            &dana(),
            lookup,
        )
    }

    #[test]
    fn a_person_whose_identity_is_linked_is_signed_in_as_that_user() {
        let lookup = Lookup {
            linked: Some(Account {
                user: user(7),
                suspended: false,
            }),
            same_email: Some(user(9)),
        };
        for mode in [
            RegistrationMode::Closed,
            RegistrationMode::Invite,
            RegistrationMode::Open,
        ] {
            assert_eq!(
                decide_signing_in(&settings(), mode, false, &lookup),
                Ok(Decision::SignIn { user: user(7) }),
                "{mode:?}: registration is not asked of somebody who has a user"
            );
        }
    }

    #[test]
    fn a_suspended_user_or_a_switched_off_provider_is_disabled() {
        let suspended = Lookup {
            linked: Some(Account {
                user: user(7),
                suspended: true,
            }),
            same_email: None,
        };
        assert_eq!(
            decide_signing_in(&settings(), RegistrationMode::Open, false, &suspended),
            Err(SignInError::Disabled)
        );
        let mut off = settings();
        off.enabled = false;
        let linked = Lookup {
            linked: Some(Account {
                user: user(7),
                suspended: false,
            }),
            same_email: None,
        };
        assert_eq!(
            decide_signing_in(&off, RegistrationMode::Open, false, &linked),
            Err(SignInError::Disabled)
        );
        assert_eq!(
            decide(
                &off,
                RegistrationMode::Open,
                Flow::Link { user: user(7) },
                &dana(),
                &Lookup::default()
            ),
            Err(SignInError::Disabled)
        );
    }

    #[test]
    fn the_list_of_domains_is_looked_at_for_a_linked_identity_too() {
        let mut narrow = settings();
        narrow.allowed_email_domains = vec!["corp.example".into()];
        let linked = Lookup {
            linked: Some(Account {
                user: user(7),
                suspended: false,
            }),
            same_email: None,
        };
        assert_eq!(
            decide_signing_in(&narrow, RegistrationMode::Open, false, &linked),
            Err(SignInError::DomainNotAllowed)
        );
        narrow.allowed_email_domains = vec!["acme.example".into()];
        assert!(decide_signing_in(&narrow, RegistrationMode::Open, false, &linked).is_ok());
        let mut unconfirmed = dana();
        unconfirmed.email_verified = false;
        assert_eq!(
            decide(
                &narrow,
                RegistrationMode::Open,
                Flow::SignIn {
                    good_invitation: false
                },
                &unconfirmed,
                &linked
            ),
            Err(SignInError::DomainNotAllowed),
            "an address the provider did not confirm is no address of the domain"
        );
    }

    #[test]
    fn a_user_with_the_same_confirmed_address_is_never_joined_by_it() {
        let lookup = Lookup {
            linked: None,
            same_email: Some(user(9)),
        };
        for mode in [
            RegistrationMode::Closed,
            RegistrationMode::Invite,
            RegistrationMode::Open,
        ] {
            assert_eq!(
                decide_signing_in(&settings(), mode, true, &lookup),
                Err(SignInError::LinkRequired),
                "{mode:?}"
            );
        }
        let mut quiet = settings();
        quiet.auto_register = false;
        assert_eq!(
            decide_signing_in(&quiet, RegistrationMode::Open, false, &lookup),
            Err(SignInError::LinkRequired),
            "the person is told to link, whether or not the provider registers"
        );
        let mut unconfirmed = dana();
        unconfirmed.email_verified = false;
        assert_eq!(
            decide(
                &settings(),
                RegistrationMode::Open,
                Flow::SignIn {
                    good_invitation: false
                },
                &unconfirmed,
                &lookup
            ),
            Ok(Decision::Register),
            "an address that is not confirmed finds nobody"
        );
    }

    #[test]
    fn a_new_person_is_registered_where_the_provider_and_the_system_allow_it() {
        let nobody = Lookup::default();
        let open = |mode, invitation| decide_signing_in(&settings(), mode, invitation, &nobody);
        assert_eq!(open(RegistrationMode::Open, false), Ok(Decision::Register));
        assert_eq!(open(RegistrationMode::Open, true), Ok(Decision::Register));
        assert_eq!(open(RegistrationMode::Invite, true), Ok(Decision::Register));
        assert_eq!(
            open(RegistrationMode::Invite, false),
            Err(SignInError::InviteRequired)
        );
        assert_eq!(
            open(RegistrationMode::Closed, true),
            Err(SignInError::RegistrationClosed)
        );
        assert_eq!(
            open(RegistrationMode::Closed, false),
            Err(SignInError::RegistrationClosed)
        );
        let mut quiet = settings();
        quiet.auto_register = false;
        assert_eq!(
            decide_signing_in(&quiet, RegistrationMode::Open, true, &nobody),
            Err(SignInError::RegistrationClosed),
            "a provider that does not register makes nobody, whatever the system allows"
        );
    }

    #[test]
    fn a_signed_in_user_links_an_identity_that_is_free_or_already_theirs_and_not_another_users() {
        let link = Flow::Link { user: user(7) };
        let decide_linking =
            |lookup: &Lookup| decide(&settings(), RegistrationMode::Closed, link, &dana(), lookup);
        assert_eq!(
            decide_linking(&Lookup::default()),
            Ok(Decision::Link { user: user(7) })
        );
        assert_eq!(
            decide_linking(&Lookup {
                linked: None,
                same_email: Some(user(9))
            }),
            Ok(Decision::Link { user: user(7) }),
            "the person is signed in: the address finds nobody else"
        );
        assert_eq!(
            decide_linking(&Lookup {
                linked: Some(Account {
                    user: user(7),
                    suspended: false
                }),
                same_email: None
            }),
            Ok(Decision::Link { user: user(7) })
        );
        assert_eq!(
            decide_linking(&Lookup {
                linked: Some(Account {
                    user: user(8),
                    suspended: false
                }),
                same_email: None
            }),
            Err(SignInError::Denied)
        );
    }

    #[test]
    fn an_identity_and_a_new_user_are_written_as_data() {
        let identity: VerifiedIdentity = serde_json::from_value(json!({
            "subject": "248289761001", "email": "dana@acme.example", "email_verified": true,
            "name": "Dana Whit", "username": null
        }))
        .unwrap();
        assert_eq!(identity.username, None);
        assert_eq!(
            serde_json::to_value(NewUser::from_identity(&identity, |_| false).unwrap()).unwrap(),
            json!({ "username": "dana", "display_name": "Dana Whit", "email": "dana@acme.example" })
        );
        assert_eq!(
            serde_json::to_value(RegistrationMode::Invite).unwrap(),
            json!("invite")
        );
    }
}
