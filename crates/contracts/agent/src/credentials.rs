//! Credential forms with their custody and lifetime, what an adapter issues and accepts, and exchange.
//!
//! A harness that runs away from its provider, to stay beside its own tools, can still run on a credential
//! it is given. What can be given is declared as data: a **form** has a name that the issuing side and the
//! accepting side both know, a custody (the credential stays with the provider and what leaves is a copy, or
//! the credential itself moves), and how long a copy is good for. An adapter lists the forms it can issue
//! from the login its harness holds and the forms it can run on ([`CredentialSupport`]); a wire protocol
//! lists the forms that can be issued from an API key (see the model contract).
//!
//! The exchange has four steps, each a method of the adapter: the provider's side issues a form
//! ([`IssuedCredential`]), the side where the harness runs installs it ([`GivenCredential`]), and for a
//! credential that moved, the same side gives it back and the provider's side takes it home again. The rules
//! that need no adapter are here: which forms two sides have in common and in what order they are taken
//! ([`usable_forms`]), how often a copy may be issued ([`may_issue`]), when one is renewed
//! ([`needs_renewal`]), and how the harness reaches the model given an arrangement ([`CredentialPath::of`]).

use orch_plugin_contract::Secret;
use orch_types::Time;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::descriptor::{ArrangementType, ProviderKind};

/// The name of the form every protocol has: the API key, as it is.
pub const API_KEY_FORM: &str = "api_key";

/// How soon after the last copy a new one may be issued, in milliseconds: an adapter that asks more often
/// fails the turn. Thirty seconds is longer than any renewal needs and shorter than the shortest copy that
/// is worth issuing.
pub const MIN_ISSUE_INTERVAL_MILLIS: i64 = 30_000;

/// Who owns a credential while it is away from its provider's runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Custody {
    /// The provider's runtime keeps the credential and stays its owner; what leaves is a copy.
    Shared,
    /// The credential moves, and the provider's runtime gives it up until it is returned.
    Transferred,
}

/// One kind of credential that can be issued and run on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct CredentialForm {
    /// Its name; the issuing and the accepting side must both know it.
    pub form: String,
    /// Whether it is a copy or the credential itself.
    pub custody: Custody,
    /// How long a copy is good for before it lapses by itself. `null`: it lasts until its owner rotates or
    /// revokes it (a key as it is, a long-lived token), and always for a transferred form, which is the
    /// login with its renewal.
    pub lifetime_secs: Option<i64>,
}

impl CredentialForm {
    /// The form every protocol has: the API key as it is, a copy that lasts.
    #[must_use]
    pub fn api_key() -> Self {
        Self {
            form: API_KEY_FORM.to_owned(),
            custody: Custody::Shared,
            lifetime_secs: None,
        }
    }

    /// Whether a copy of it lapses by itself.
    #[must_use]
    pub const fn lapses(&self) -> bool {
        matches!(self.custody, Custody::Shared) && self.lifetime_secs.is_some()
    }

    /// What is wrong with the declaration, if anything: a transferred form has no lifetime, because it is
    /// the login with its renewal; a lifetime is positive; a form has a name.
    pub fn check(&self) -> Result<(), FormFault> {
        if self.form.is_empty() {
            return Err(FormFault::NoName);
        }
        if self.custody == Custody::Transferred && self.lifetime_secs.is_some() {
            return Err(FormFault::TransferredWithLifetime(self.form.clone()));
        }
        if self.lifetime_secs.is_some_and(|secs| secs <= 0) {
            return Err(FormFault::LifetimeNotPositive(self.form.clone()));
        }
        Ok(())
    }
}

/// What is wrong with a form's declaration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FormFault {
    /// It has no name.
    #[error("a credential form needs a name")]
    NoName,
    /// A transferred form states a lifetime; it lasts as the login does.
    #[error("form {0:?} is transferred, so it has no lifetime of its own")]
    TransferredWithLifetime(String),
    /// A lifetime is a positive number of seconds.
    #[error("form {0:?} has a lifetime that is not positive")]
    LifetimeNotPositive(String),
}

/// The forms an adapter deals in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct CredentialSupport {
    /// The forms it can produce, in the provider's runtime, from the login its harness holds there.
    #[serde(default)]
    pub issues: Vec<CredentialForm>,
    /// The names of the forms its harness can run on.
    #[serde(default)]
    pub accepts: Vec<String>,
}

/// The forms that `issued` and `accepted` have in common, with those of `custody` only when it is given,
/// in the order they are taken: a form that lapses by itself comes before one that lasts, and otherwise
/// the order of `issued` holds.
#[must_use]
pub fn usable_forms(
    issued: &[CredentialForm],
    accepted: &[String],
    custody: Option<Custody>,
) -> Vec<CredentialForm> {
    let mut usable: Vec<CredentialForm> = issued
        .iter()
        .filter(|form| custody.is_none_or(|wanted| form.custody == wanted))
        .filter(|form| accepted.contains(&form.form))
        .cloned()
        .collect();
    usable.sort_by_key(|form| !form.lapses());
    usable
}

/// A credential produced for a form, in the provider's runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct IssuedCredential {
    /// The form it is in.
    pub form: String,
    /// The value. Never more than the form declares: for an access token, never the refresh token.
    pub credential: Secret,
    /// When it lapses, where it does.
    pub expires_at: Option<Time>,
}

/// Where a given credential points the harness, when it points it anywhere other than at its vendor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct UpstreamTarget {
    /// The wire protocol of the endpoint.
    pub protocol: String,
    /// The endpoint's base address.
    pub base_url: String,
}

/// A credential as the side where the harness runs is given it, and as it is returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GivenCredential {
    /// The form it is in.
    pub form: String,
    /// The value.
    pub credential: Secret,
    /// When it lapses, where it does.
    pub expires_at: Option<Time>,
    /// The endpoint it is for, when it is a key of an API provider.
    pub upstream: Option<UpstreamTarget>,
}

impl IssuedCredential {
    /// This credential as it is given to the harness, for the endpoint `upstream` if it has one.
    #[must_use]
    pub fn into_given(self, upstream: Option<UpstreamTarget>) -> GivenCredential {
        GivenCredential {
            form: self.form,
            credential: self.credential,
            expires_at: self.expires_at,
            upstream,
        }
    }
}

/// Why a harness that holds a copy says it needs another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialNeed {
    /// It has none.
    Missing,
    /// What it has is about to lapse.
    Expiring,
    /// The upstream refused what it has.
    Rejected,
}

/// How the harness reaches the model in an arrangement. It follows from the arrangement's type and from
/// what the provider is, and is never chosen by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialPath {
    /// The harness uses the provider's login directly, in the home where that login is kept.
    Native,
    /// The harness calls Orch's forwarder, which adds the key.
    Forwarded,
    /// The harness runs on a copy of the credential; the provider's runtime keeps the original.
    Shared,
    /// The credential itself moves to the environment, which holds it until it is returned.
    Handover,
}

impl CredentialPath {
    /// The path of an arrangement of type `arrangement` on a provider of kind `provider`.
    ///
    /// `holds_login` is whether the profile is the one whose harness holds the provider's login; a second
    /// profile of the same harness kind that runs in the provider's own runtime on the same subscription
    /// works on a copy (path `shared`), though nothing leaves.
    #[must_use]
    pub const fn of(
        arrangement: ArrangementType,
        provider: ProviderKind,
        holds_login: bool,
    ) -> Self {
        match (arrangement, provider) {
            (ArrangementType::CredentialShared, _) => Self::Shared,
            (ArrangementType::HandedOver, _) => Self::Handover,
            (
                ArrangementType::Separated | ArrangementType::Together,
                ProviderKind::Subscription,
            ) => {
                if holds_login {
                    Self::Native
                } else {
                    Self::Shared
                }
            }
            (
                ArrangementType::Separated
                | ArrangementType::KeySeparated
                | ArrangementType::Together,
                _,
            ) => Self::Forwarded,
        }
    }
}

/// Whether a new copy may be issued now: not while one is in flight for the session, and not within
/// [`MIN_ISSUE_INTERVAL_MILLIS`] of the last one.
#[must_use]
pub fn may_issue(in_flight: bool, last_issue: Option<Time>, now: Time) -> bool {
    !in_flight
        && last_issue
            .is_none_or(|last| now.unix_millis() - last.unix_millis() >= MIN_ISSUE_INTERVAL_MILLIS)
}

/// Whether a copy that lapses at `expires_at` is to be replaced at `now`: it has less than `margin_secs`
/// left. A copy that does not lapse is never renewed.
#[must_use]
pub fn needs_renewal(expires_at: Option<Time>, now: Time, margin_secs: i64) -> bool {
    expires_at
        .is_some_and(|end| end.unix_millis() - now.unix_millis() < margin_secs.saturating_mul(1000))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn time(text: &str) -> Time {
        text.parse().unwrap()
    }

    fn form(name: &str, custody: Custody, lifetime_secs: Option<i64>) -> CredentialForm {
        CredentialForm {
            form: name.to_owned(),
            custody,
            lifetime_secs,
        }
    }

    #[test]
    fn a_form_reads_as_the_spec_writes_it() {
        let form: CredentialForm = serde_json::from_value(
            json!({ "form": "access_token", "custody": "shared", "lifetime_secs": 3600 }),
        )
        .unwrap();
        assert!(form.lapses());
        let login: CredentialForm = serde_json::from_value(
            json!({ "form": "login", "custody": "transferred", "lifetime_secs": null }),
        )
        .unwrap();
        assert!(!login.lapses());
        assert_eq!(CredentialForm::api_key().form, API_KEY_FORM);
    }

    #[test]
    fn a_form_that_lapses_is_taken_before_one_that_lasts() {
        let issued = [
            form("oauth_token", Custody::Shared, None),
            form("access_token", Custody::Shared, Some(3600)),
            form("login", Custody::Transferred, None),
        ];
        let accepted = ["access_token", "oauth_token", "login"].map(String::from);
        let names =
            |forms: Vec<CredentialForm>| forms.into_iter().map(|f| f.form).collect::<Vec<_>>();
        assert_eq!(
            names(usable_forms(&issued, &accepted, Some(Custody::Shared))),
            ["access_token", "oauth_token"]
        );
        assert_eq!(
            names(usable_forms(&issued, &accepted, Some(Custody::Transferred))),
            ["login"]
        );
        assert_eq!(
            names(usable_forms(&issued, &["login".to_owned()], None)),
            ["login"]
        );
    }

    #[test]
    fn a_transferred_form_has_no_lifetime_and_a_lifetime_is_positive() {
        assert_eq!(
            form("login", Custody::Transferred, Some(60)).check(),
            Err(FormFault::TransferredWithLifetime("login".into()))
        );
        assert_eq!(
            form("t", Custody::Shared, Some(0)).check(),
            Err(FormFault::LifetimeNotPositive("t".into()))
        );
        assert_eq!(
            form("", Custody::Shared, None).check(),
            Err(FormFault::NoName)
        );
        assert_eq!(form("t", Custody::Shared, Some(1)).check(), Ok(()));
    }

    #[test]
    fn the_credential_path_follows_from_the_arrangement_and_the_provider() {
        use ArrangementType::*;
        use ProviderKind::*;
        assert_eq!(
            CredentialPath::of(Separated, Subscription, true),
            CredentialPath::Native
        );
        assert_eq!(
            CredentialPath::of(Together, Subscription, true),
            CredentialPath::Native
        );
        assert_eq!(
            CredentialPath::of(Separated, Api, true),
            CredentialPath::Forwarded
        );
        assert_eq!(
            CredentialPath::of(KeySeparated, Api, true),
            CredentialPath::Forwarded
        );
        assert_eq!(
            CredentialPath::of(Together, Api, false),
            CredentialPath::Forwarded
        );
        assert_eq!(
            CredentialPath::of(CredentialShared, Subscription, true),
            CredentialPath::Shared
        );
        assert_eq!(
            CredentialPath::of(HandedOver, Subscription, true),
            CredentialPath::Handover
        );
        assert_eq!(
            CredentialPath::of(Separated, Subscription, false),
            CredentialPath::Shared,
            "a second profile of one harness kind runs on a copy inside the provider's runtime"
        );
    }

    #[test]
    fn a_copy_is_not_issued_while_one_is_in_flight_nor_within_thirty_seconds_of_the_last() {
        let last = time("2026-10-11T10:00:00.000Z");
        assert!(may_issue(false, None, last));
        assert!(!may_issue(true, None, last));
        assert!(!may_issue(
            false,
            Some(last),
            time("2026-10-11T10:00:29.999Z")
        ));
        assert!(may_issue(
            false,
            Some(last),
            time("2026-10-11T10:00:30.000Z")
        ));
    }

    #[test]
    fn a_copy_is_renewed_when_less_than_the_margin_is_left_and_a_lasting_one_never() {
        let now = time("2026-10-11T10:00:00.000Z");
        let end = Some(time("2026-10-11T10:05:00.000Z"));
        assert!(needs_renewal(end, now, 600));
        assert!(!needs_renewal(end, now, 300));
        assert!(!needs_renewal(end, now, 60));
        assert!(!needs_renewal(None, now, 600));
    }

    #[test]
    fn an_issued_credential_is_written_as_its_text_and_never_printed() {
        let issued = IssuedCredential {
            form: "access_token".into(),
            credential: Secret::new("sk-ant-oat-1"),
            expires_at: Some(time("2026-10-11T11:00:00.000Z")),
        };
        assert_eq!(
            serde_json::to_value(&issued).unwrap()["credential"],
            json!("sk-ant-oat-1")
        );
        assert!(!format!("{issued:?}").contains("sk-ant"));
        let given = issued.into_given(Some(UpstreamTarget {
            protocol: "anthropic-messages".into(),
            base_url: "https://api.example.com".into(),
        }));
        assert_eq!(given.form, "access_token");
    }
}
