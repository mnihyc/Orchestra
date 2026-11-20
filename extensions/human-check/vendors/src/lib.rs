//! The first human-check backends: hCaptcha, and reCAPTCHA of Google Cloud Fraud Defense in its two ways, `siteverify`
//! and the Assessments API.
//!
//! Each backend asks its vendor about one token and reports what the vendor said as facts
//! ([`orch_human_check_contract::Facts`]); the decision is the contract's. The one way a backend calls a vendor is a
//! [`Transport`], so that the tests of each backend are the literal answers of the vendor, and the API's tests are a
//! scripted vendor, with no network.

#![forbid(unsafe_code)]

pub mod edge;
pub mod hcaptcha;
pub mod recaptcha;
pub mod recaptcha_assessments;
pub mod recaptcha_siteverify;
pub mod transport;

use std::sync::Arc;

use orch_human_check_contract::{EdgeHosts, HCAPTCHA, HumanCheckProvider, RECAPTCHA};
pub use edge::{ScriptedEdge, nginx_line_of, policy_of};
pub use orch_human_check_contract::edge::{EdgeAnswer, EdgeReader, Lack, lacking};
pub use orch_human_check_contract::{Unreachable, Why};
pub use transport::{Body, Http, Reply, Scripted, Transport};

/// The backends this build carries, speaking through `transport`.
#[must_use]
pub fn builtin(transport: &Arc<dyn Transport>) -> Vec<Arc<dyn HumanCheckProvider>> {
    vec![
        Arc::new(hcaptcha::HCaptcha::new(Arc::clone(transport))),
        Arc::new(recaptcha_siteverify::RecaptchaSiteVerify::new(Arc::clone(
            transport,
        ))),
        Arc::new(recaptcha_assessments::RecaptchaAssessments::new(
            Arc::clone(transport),
        )),
    ]
}

/// The hosts a Content-Security-Policy has to allow for the backend `kind`, without a backend being made: for the
/// doctor and the edge's own table, which do not speak to a vendor.
#[must_use]
pub fn edge_hosts_of(kind: &str, alternate: bool) -> Option<EdgeHosts> {
    match kind {
        HCAPTCHA => Some(hcaptcha::EDGE_HOSTS),
        RECAPTCHA => Some(if alternate {
            recaptcha::EDGE_HOSTS_ALTERNATE
        } else {
            recaptcha::EDGE_HOSTS
        }),
        _ => None,
    }
}

/// The most characters of a vendor's own word that an error keeps: enough to read, too few to carry anything else.
pub(crate) const WORD_MAX: usize = 60;

/// A vendor's word (an error code, a status name) cut to what an audit line may hold, with only the characters such
/// a word is made of.
pub(crate) fn word(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(WORD_MAX)
        .collect()
}
