//! HumanCheckProvider trait seam; hCaptcha and reCAPTCHA of Google Cloud Fraud Defense are the first backends.
//!
//! A human check is the proof a person gives, before something is made or mailed for them, that they are not a
//! program. The vendor's widget gives the browser a token; the API asks the vendor about it. What differs between
//! vendors is only how they are asked and how they word their answer, and that is all a backend
//! ([`HumanCheckProvider`]) does: it asks, and reports what the vendor said ([`Facts`]), or that the vendor could not
//! be asked ([`Unreachable`]). It does not decide.
//!
//! The decision is made here, once, for every backend ([`judge`]):
//!
//! 1. A vendor that could not be asked has not judged the person: the verdict is [`Verdict::Unavailable`], and the
//!    API's setting says whether the person is let in and the event recorded, or refused.
//! 2. A token the vendor refused fails with the vendor's word for it: missing, invalid, old or used before.
//! 3. A token the vendor accepted must still be for this act (where the backend can say which act it was made for),
//!    solved on this installation's own address (where the backend can say where), and score at least what the
//!    system asks (where the backend gives a score).
//!
//! A further vendor, or a further way of asking one (reCAPTCHA is asked through `siteverify` or the Assessments API), is
//! one more implementation of the trait with those three answers; nothing in this decision, in the settings or in the
//! pages changes with it.

#![forbid(unsafe_code)]

pub mod edge;

use std::net::IpAddr;
use std::time::Duration;

use async_trait::async_trait;
pub use orch_plugin_contract::Secret;

/// The value of the `human_check.provider` setting that selects hCaptcha, which is also the kind of its backend.
pub const HCAPTCHA: &str = "hcaptcha";
/// The value of the `human_check.provider` setting that selects Google Cloud Fraud Defense, whose widget, answers and
/// notice are still called reCAPTCHA. It has two ways of being asked about a token ([`HumanCheckProvider::way`]).
pub const RECAPTCHA: &str = "recaptcha";
/// The way of asking Google with the site key and the secret key the console shows: `siteverify`.
pub const SITEVERIFY: &str = "siteverify";
/// The way of asking Google through a Google Cloud project and its API key, which Google recommends: `assessments`.
pub const ASSESSMENTS: &str = "assessments";

/// What a person is doing when they are asked to prove that they are a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Act {
    /// Signing in with a proof: a password, a mailed code or the second step that follows them.
    SignIn,
    /// Asking for a sign-in code to be mailed.
    EmailCode,
    /// Making an account.
    Register,
    /// Asking for a link to reset a password to be mailed.
    Recovery,
    /// Asking for the confirmation link of an address to be mailed again.
    Resend,
}

impl Act {
    /// Every act, in the order the screens meet them.
    pub const ALL: [Act; 5] = [
        Act::SignIn,
        Act::EmailCode,
        Act::Register,
        Act::Recovery,
        Act::Resend,
    ];

    /// The word the setting and the page know it by.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Act::SignIn => "sign_in",
            Act::EmailCode => "email_code",
            Act::Register => "register",
            Act::Recovery => "recovery",
            Act::Resend => "resend",
        }
    }

    /// The act a word names, if it names one.
    #[must_use]
    pub fn named(word: &str) -> Option<Act> {
        Self::ALL.into_iter().find(|act| act.as_str() == word)
    }

    /// The name a vendor that binds a token to an act is told: letters and underscores only, the same for everybody
    /// (an action must not be user-specific).
    #[must_use]
    pub const fn vendor_action(self) -> &'static str {
        match self {
            Act::SignIn => "orch_sign_in",
            Act::EmailCode => "orch_email_code",
            Act::Register => "orch_register",
            Act::Recovery => "orch_recovery",
            Act::Resend => "orch_resend",
        }
    }
}

/// What a backend is given of the installation: the site key, the project where the vendor has one, and the secret.
#[derive(Debug, Clone)]
pub struct Config {
    /// The public key of the widget.
    pub site_key: String,
    /// The vendor's own project, where it has such a thing (Google Cloud's); empty otherwise.
    pub project_id: String,
    /// The secret, or the key that stands for it: used for one call and never shown.
    pub secret: Secret,
    /// How long a call to the vendor may take.
    pub timeout: Duration,
}

/// One token to ask the vendor about.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    /// What the widget gave the browser.
    pub token: &'a str,
    /// What the person is doing, which a vendor that binds tokens to acts is told.
    pub act: Act,
    /// The address the person's request came from, as the trusted-proxy rules resolve it.
    pub remote_ip: Option<IpAddr>,
    /// How the person's browser calls itself, for the vendor that asks.
    pub user_agent: Option<&'a str>,
    /// The installation's keys.
    pub config: &'a Config,
}

/// Why a vendor refused a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejected {
    /// There was none.
    Missing,
    /// It is not one the vendor made, or not one for this key.
    Invalid,
    /// It is too old.
    Expired,
    /// It was verified once already.
    Replayed,
}

/// What a vendor said of a token, and nothing it did not say.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Facts {
    /// Why the vendor refused the token; none where it accepted it.
    pub rejected: Option<Rejected>,
    /// The site the widget was solved on, as the vendor reports it.
    pub hostname: Option<String>,
    /// The act the token was made for, as the vendor reports it.
    pub action: Option<String>,
    /// How human the interaction looked, from 0.0 (a program) to 1.0 (a person), where the vendor gives one.
    pub score: Option<f64>,
    /// The vendor's words for its score.
    pub reasons: Vec<String>,
}

impl Facts {
    /// A token the vendor accepted.
    #[must_use]
    pub fn accepted() -> Self {
        Self::default()
    }

    /// A token the vendor refused.
    #[must_use]
    pub fn refused(why: Rejected) -> Self {
        Self {
            rejected: Some(why),
            ..Self::default()
        }
    }
}

/// How a vendor could not be asked, in the word an administrator reads in the audit and in the doctor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The vendor did not answer in time.
    Timeout,
    /// The connection could not be made.
    Connect,
    /// The vendor answered with a status that says it is not working, or that the request was refused as a request.
    Status,
    /// The answer could not be read.
    Unreadable,
    /// The vendor does not accept the secret or the key.
    Secret,
    /// The vendor does not know the site key or the project, or answered something this side does not know.
    Setup,
    /// The vendor was not asked: the web edge's content policy does not let a page reach its widget, so nobody could
    /// have a token to be asked about.
    Edge,
}

impl Why {
    /// The word for it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Why::Timeout => "timeout",
            Why::Connect => "connect",
            Why::Status => "status",
            Why::Unreadable => "unreadable",
            Why::Secret => "secret",
            Why::Setup => "setup",
            Why::Edge => "edge",
        }
    }
}

/// A vendor that could not be asked. It carries no token and no secret: it is written to the audit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreachable {
    /// How.
    pub why: Why,
    /// What else is worth knowing, in plain words (a status, a vendor's error word).
    pub detail: String,
}

impl Unreachable {
    /// An unreachable vendor, with a few words about it.
    #[must_use]
    pub fn new(why: Why, detail: impl Into<String>) -> Self {
        Self {
            why,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            f.write_str(self.why.word())
        } else {
            write!(f, "{}: {}", self.why.word(), self.detail)
        }
    }
}

impl std::error::Error for Unreachable {}

/// The hosts a Content-Security-Policy has to let a page's visitors reach for a vendor's widget to work, by directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EdgeHosts {
    /// Where the vendor's script is loaded from.
    pub script: &'static [&'static str],
    /// Where the vendor's frames are loaded from.
    pub frame: &'static [&'static str],
    /// Where the vendor's script reaches.
    pub connect: &'static [&'static str],
    /// Where the vendor's stylesheets are loaded from.
    pub style: &'static [&'static str],
}

/// What a backend can say of a token, which decides what the decision may ask of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Traits {
    /// Whether the vendor reports the act a token was made for.
    pub binds_action: bool,
    /// Whether the vendor reports where a token was solved in a way that can be believed.
    pub binds_site: bool,
    /// Whether the vendor gives a score that the setting may bound.
    pub scores: bool,
}

/// One vendor, as the API speaks to it.
#[async_trait]
pub trait HumanCheckProvider: Send + Sync + 'static {
    /// The name of this backend, for the records.
    fn kind(&self) -> &'static str;

    /// The value of `human_check.provider` that selects it: the service whose widget the pages show. Two backends
    /// may speak for one service in different ways.
    fn service(&self) -> &'static str;

    /// The value of `human_check.verification` that selects it among the backends of its service; empty where the
    /// service has but one way.
    fn way(&self) -> &'static str {
        ""
    }

    /// What this vendor can say of a token.
    fn traits(&self) -> Traits;

    /// The hosts a page that shows this vendor's widget needs a Content-Security-Policy to allow; the alternate
    /// address is the one a vendor offers where its usual one cannot be reached.
    fn edge_hosts(&self, alternate: bool) -> EdgeHosts;

    /// Asks the vendor about one token, and reports what it said. Every failure to get an answer is
    /// [`Unreachable`]; a token the vendor refuses is an answer.
    async fn verify(&self, request: &Request<'_>) -> Result<Facts, Unreachable>;
}

/// What the decision asks of an answer.
#[derive(Debug, Clone, Copy)]
pub struct Expectation<'a> {
    /// What the person is doing.
    pub act: Act,
    /// The hosts this installation answers at, in lower case; none where it does not know them.
    pub hosts: &'a [String],
    /// The least score that passes, in whole percent: 50 is 0.5.
    pub least_score_percent: u8,
}

/// Why a token that the vendor answered for does not pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// There was no token.
    Missing,
    /// The vendor refused it.
    Invalid,
    /// It is too old.
    Expired,
    /// It had been used.
    Replayed,
    /// It was made for another act.
    WrongAct,
    /// It was solved on another site.
    WrongSite,
    /// It scored less than the system asks.
    LowScore,
}

impl Failure {
    /// The word an answer carries for it.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Failure::Missing => "missing",
            Failure::Invalid => "invalid",
            Failure::Expired => "expired",
            Failure::Replayed => "replayed",
            Failure::WrongAct => "wrong_act",
            Failure::WrongSite => "wrong_site",
            Failure::LowScore => "low_score",
        }
    }
}

/// What is decided of one token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The person is let on.
    Pass,
    /// The vendor judged, and the person did not pass.
    Fail(Failure),
    /// The vendor did not judge: it could not be asked.
    Unavailable(Unreachable),
}

/// The host of an address, in lower case without a trailing dot: where an installation answers.
#[must_use]
pub fn host_of(address: &str) -> Option<String> {
    let parsed = url::Url::parse(address.trim()).ok()?;
    let host = parsed.host_str()?.trim_end_matches('.').to_lowercase();
    (!host.is_empty()).then_some(host)
}

/// The hosts an installation answers at: its public address and the others that name it.
#[must_use]
pub fn hosts_of<'a>(addresses: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for address in addresses {
        if let Some(host) = host_of(address)
            && !hosts.contains(&host)
        {
            hosts.push(host);
        }
    }
    hosts
}

/// Decides what a vendor's answer to one token comes to.
#[must_use]
pub fn judge(
    traits: Traits,
    answer: Result<Facts, Unreachable>,
    expect: &Expectation<'_>,
) -> Verdict {
    let facts = match answer {
        Ok(facts) => facts,
        Err(unreachable) => return Verdict::Unavailable(unreachable),
    };
    if let Some(rejected) = facts.rejected {
        return Verdict::Fail(match rejected {
            Rejected::Missing => Failure::Missing,
            Rejected::Invalid => Failure::Invalid,
            Rejected::Expired => Failure::Expired,
            Rejected::Replayed => Failure::Replayed,
        });
    }
    if traits.binds_action && facts.action.as_deref() != Some(expect.act.vendor_action()) {
        return Verdict::Fail(Failure::WrongAct);
    }
    if traits.binds_site && !expect.hosts.is_empty() {
        let solved = facts
            .hostname
            .as_deref()
            .map(|host| host.trim_end_matches('.').to_lowercase());
        if !solved.is_some_and(|host| expect.hosts.contains(&host)) {
            return Verdict::Fail(Failure::WrongSite);
        }
    }
    if traits.scores {
        let Some(score) = facts.score else {
            // A scoring vendor that gave no score has not judged.
            return Verdict::Unavailable(Unreachable::new(
                Why::Setup,
                "the answer carries no score",
            ));
        };
        if (score * 100.0).round() < f64::from(expect.least_score_percent) {
            return Verdict::Fail(Failure::LowScore);
        }
    }
    Verdict::Pass
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use std::sync::Mutex;

    /// A backend that answers what it was told to, once each, and notes what it was asked.
    struct Scripted {
        traits: Traits,
        answers: Mutex<Vec<Result<Facts, Unreachable>>>,
        asked: Mutex<Vec<(String, Act)>>,
    }

    impl Scripted {
        fn new(traits: Traits, answers: Vec<Result<Facts, Unreachable>>) -> Self {
            Self {
                traits,
                answers: Mutex::new(answers),
                asked: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl HumanCheckProvider for Scripted {
        fn kind(&self) -> &'static str {
            "scripted"
        }
        fn service(&self) -> &'static str {
            "scripted"
        }
        fn traits(&self) -> Traits {
            self.traits
        }
        fn edge_hosts(&self, _alternate: bool) -> EdgeHosts {
            EdgeHosts {
                script: &[],
                frame: &[],
                connect: &[],
                style: &[],
            }
        }
        async fn verify(&self, request: &Request<'_>) -> Result<Facts, Unreachable> {
            self.asked
                .lock()
                .unwrap()
                .push((request.token.to_owned(), request.act));
            let mut answers = self.answers.lock().unwrap();
            if answers.is_empty() {
                return Err(Unreachable::new(Why::Setup, "nothing was scripted"));
            }
            answers.remove(0)
        }
    }

    const BOTH: Traits = Traits {
        binds_action: true,
        binds_site: true,
        scores: true,
    };
    const NEITHER: Traits = Traits {
        binds_action: false,
        binds_site: false,
        scores: false,
    };

    fn config() -> Config {
        Config {
            site_key: "site".to_owned(),
            project_id: "project".to_owned(),
            secret: Secret::new("the-secret"),
            timeout: Duration::from_secs(5),
        }
    }

    fn ask(provider: &Scripted, token: &str, act: Act, hosts: &[String], least: u8) -> Verdict {
        let config = config();
        let answer = block_on(provider.verify(&Request {
            token,
            act,
            remote_ip: None,
            user_agent: None,
            config: &config,
        }));
        judge(
            provider.traits(),
            answer,
            &Expectation {
                act,
                hosts,
                least_score_percent: least,
            },
        )
    }

    fn here() -> Vec<String> {
        vec!["orch.example".to_owned()]
    }

    fn good(action: &str, host: &str, score: f64) -> Facts {
        Facts {
            action: Some(action.to_owned()),
            hostname: Some(host.to_owned()),
            score: Some(score),
            ..Facts::accepted()
        }
    }

    #[test]
    fn a_token_the_vendor_accepts_for_the_act_and_the_site_passes() {
        let provider = Scripted::new(BOTH, vec![Ok(good("orch_sign_in", "orch.example", 0.9))]);
        assert_eq!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Pass
        );
        assert_eq!(
            provider.asked.lock().unwrap().as_slice(),
            &[("token".to_owned(), Act::SignIn)]
        );
    }

    #[test]
    fn a_token_the_vendor_refuses_fails_with_its_reason() {
        for (rejected, failure) in [
            (Rejected::Missing, Failure::Missing),
            (Rejected::Invalid, Failure::Invalid),
            (Rejected::Expired, Failure::Expired),
            (Rejected::Replayed, Failure::Replayed),
        ] {
            let provider = Scripted::new(BOTH, vec![Ok(Facts::refused(rejected))]);
            assert_eq!(
                ask(&provider, "token", Act::Register, &here(), 50),
                Verdict::Fail(failure)
            );
        }
    }

    #[test]
    fn a_vendor_that_cannot_be_reached_is_unavailable_and_never_a_failure_of_the_person() {
        for why in [Why::Timeout, Why::Connect, Why::Status, Why::Unreadable] {
            let provider = Scripted::new(BOTH, vec![Err(Unreachable::new(why, "down"))]);
            assert_eq!(
                ask(&provider, "token", Act::SignIn, &here(), 50),
                Verdict::Unavailable(Unreachable::new(why, "down"))
            );
        }
    }

    #[test]
    fn a_wrong_secret_is_unavailable_and_not_a_failure() {
        let provider = Scripted::new(
            BOTH,
            vec![Err(Unreachable::new(
                Why::Secret,
                "the vendor does not accept the key",
            ))],
        );
        assert!(matches!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Unavailable(Unreachable {
                why: Why::Secret,
                ..
            })
        ));
    }

    #[test]
    fn a_replayed_token_is_refused_the_second_time() {
        let provider = Scripted::new(
            BOTH,
            vec![
                Ok(good("orch_sign_in", "orch.example", 0.9)),
                Ok(Facts::refused(Rejected::Replayed)),
            ],
        );
        assert_eq!(
            ask(&provider, "same", Act::SignIn, &here(), 50),
            Verdict::Pass
        );
        assert_eq!(
            ask(&provider, "same", Act::SignIn, &here(), 50),
            Verdict::Fail(Failure::Replayed)
        );
    }

    #[test]
    fn an_old_token_is_expired() {
        let provider = Scripted::new(BOTH, vec![Ok(Facts::refused(Rejected::Expired))]);
        assert_eq!(
            ask(&provider, "token", Act::Recovery, &here(), 50),
            Verdict::Fail(Failure::Expired)
        );
        assert_eq!(Failure::Expired.reason(), "expired");
    }

    #[test]
    fn a_token_made_for_another_act_is_refused() {
        let provider = Scripted::new(
            BOTH,
            vec![
                Ok(good("orch_register", "orch.example", 0.9)),
                Ok(Facts {
                    action: None,
                    ..good("", "orch.example", 0.9)
                }),
            ],
        );
        assert_eq!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Fail(Failure::WrongAct)
        );
        assert_eq!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Fail(Failure::WrongAct)
        );
    }

    #[test]
    fn a_token_solved_on_another_site_is_refused() {
        let provider = Scripted::new(
            BOTH,
            vec![
                Ok(good("orch_sign_in", "evil.example", 0.9)),
                Ok(Facts {
                    hostname: None,
                    ..good("orch_sign_in", "", 0.9)
                }),
                Ok(good("orch_sign_in", "ORCH.example.", 0.9)),
            ],
        );
        assert_eq!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Fail(Failure::WrongSite)
        );
        assert_eq!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Fail(Failure::WrongSite)
        );
        // The same name in another case, with the dot of a rooted name, is the same site.
        assert_eq!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Pass
        );
    }

    #[test]
    fn an_installation_that_does_not_know_its_own_address_does_not_refuse_a_site() {
        let provider = Scripted::new(
            BOTH,
            vec![Ok(good("orch_sign_in", "anywhere.example", 0.9))],
        );
        assert_eq!(ask(&provider, "token", Act::SignIn, &[], 50), Verdict::Pass);
    }

    #[test]
    fn a_score_below_the_setting_fails_and_one_at_it_passes() {
        let provider = Scripted::new(
            BOTH,
            vec![
                Ok(good("orch_sign_in", "orch.example", 0.3)),
                Ok(good("orch_sign_in", "orch.example", 0.5)),
                Ok(good("orch_sign_in", "orch.example", 0.7)),
            ],
        );
        assert_eq!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Fail(Failure::LowScore)
        );
        assert_eq!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Pass
        );
        assert_eq!(
            ask(&provider, "token", Act::SignIn, &here(), 71),
            Verdict::Fail(Failure::LowScore)
        );
    }

    #[test]
    fn a_scoring_vendor_that_gives_no_score_has_not_judged() {
        let provider = Scripted::new(
            BOTH,
            vec![Ok(Facts {
                score: None,
                ..good("orch_sign_in", "orch.example", 0.0)
            })],
        );
        assert!(matches!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Unavailable(_)
        ));
    }

    #[test]
    fn a_backend_that_cannot_bind_actions_is_not_failed_for_having_none() {
        let provider = Scripted::new(NEITHER, vec![Ok(Facts::accepted())]);
        assert_eq!(
            ask(&provider, "token", Act::Resend, &here(), 100),
            Verdict::Pass
        );
    }

    #[test]
    fn a_vendors_hostname_that_cannot_be_believed_is_not_asked_for() {
        let provider = Scripted::new(
            Traits {
                binds_site: false,
                ..NEITHER
            },
            vec![Ok(Facts {
                hostname: Some("not-provided".to_owned()),
                ..Facts::accepted()
            })],
        );
        assert_eq!(
            ask(&provider, "token", Act::SignIn, &here(), 50),
            Verdict::Pass
        );
    }

    #[test]
    fn the_hosts_of_an_installation_are_its_public_address_and_the_others_that_name_it() {
        assert_eq!(
            hosts_of([
                "https://Orch.Example/",
                "https://orch.example:8443/app",
                "http://10.0.0.5:5180",
                "not an address",
            ]),
            vec!["orch.example".to_owned(), "10.0.0.5".to_owned()]
        );
        assert_eq!(host_of(""), None);
    }

    #[test]
    fn every_act_has_a_word_and_a_vendor_action_that_a_vendor_accepts() {
        for act in Act::ALL {
            assert_eq!(Act::named(act.as_str()), Some(act));
            assert!(
                act.vendor_action()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '/'),
                "{}",
                act.vendor_action()
            );
        }
        assert_eq!(Act::named("setup"), None);
    }

    #[test]
    fn an_unreachable_vendor_and_a_secret_show_nothing_of_the_secret() {
        let config = config();
        assert!(!format!("{config:?}").contains("the-secret"));
        let unreachable = Unreachable::new(Why::Status, "status 503");
        assert_eq!(unreachable.to_string(), "status: status 503");
    }
}
