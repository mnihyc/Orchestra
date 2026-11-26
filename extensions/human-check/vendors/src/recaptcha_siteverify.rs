//! reCAPTCHA through `siteverify`: the site key and the secret key that Google's console shows.
//!
//! This is what a key made today still gets, and what the console itself says to move from ("if you are using
//! SiteVerify, we recommend migrating to CreateAssessment"). For a score-based key the answer carries the score, the
//! action the widget was run with and the site it was solved on; for a checkbox key it carries neither, which the
//! decision then refuses, so a checkbox key is not a key this backend accepts. The answer's error words are Google's
//! own.

use std::sync::Arc;

use async_trait::async_trait;
use orch_human_check_contract::{
    EdgeHosts, Facts, HumanCheckProvider, RECAPTCHA, Rejected, Request, SITEVERIFY, Traits,
    Unreachable, Why,
};
use serde_json::Value;

use crate::recaptcha::{EDGE_HOSTS, EDGE_HOSTS_ALTERNATE};
use crate::transport::{Body, Reply, Transport};
use crate::word;

/// Where a token is verified.
pub const VERIFY_URL: &str = "https://www.google.com/recaptcha/api/siteverify";

/// reCAPTCHA, asked through `siteverify`.
pub struct RecaptchaSiteVerify {
    transport: Arc<dyn Transport>,
}

impl RecaptchaSiteVerify {
    /// The backend, speaking through `transport`.
    #[must_use]
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl HumanCheckProvider for RecaptchaSiteVerify {
    fn kind(&self) -> &'static str {
        "recaptcha-siteverify"
    }

    fn service(&self) -> &'static str {
        RECAPTCHA
    }

    fn way(&self) -> &'static str {
        SITEVERIFY
    }

    fn traits(&self) -> Traits {
        Traits {
            binds_action: true,
            binds_site: true,
            scores: true,
        }
    }

    fn edge_hosts(&self, alternate: bool) -> EdgeHosts {
        if alternate {
            EDGE_HOSTS_ALTERNATE
        } else {
            EDGE_HOSTS
        }
    }

    async fn verify(&self, request: &Request<'_>) -> Result<Facts, Unreachable> {
        let mut form = vec![
            (
                "secret".to_owned(),
                request.config.secret.expose().to_owned(),
            ),
            ("response".to_owned(), request.token.to_owned()),
        ];
        if let Some(ip) = request.remote_ip {
            form.push(("remoteip".to_owned(), ip.to_string()));
        }
        let reply = self
            .transport
            .post(VERIFY_URL, Body::Form(form), request.config.timeout)
            .await?;
        read(&reply)
    }
}

/// What the answer of `siteverify` says of the token.
pub(crate) fn read(reply: &Reply) -> Result<Facts, Unreachable> {
    let Ok(document) = serde_json::from_slice::<Value>(&reply.body) else {
        return Err(if (200..300).contains(&reply.status) {
            Unreachable::new(Why::Unreadable, "the answer is not JSON")
        } else {
            Unreachable::new(Why::Status, format!("status {}", reply.status))
        });
    };
    if document.get("success").and_then(Value::as_bool) == Some(true) {
        let text = |name: &str| {
            document
                .get(name)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        return Ok(Facts {
            rejected: None,
            hostname: text("hostname"),
            action: text("action"),
            score: document.get("score").and_then(Value::as_f64),
            reasons: Vec::new(),
        });
    }
    let codes: Vec<&str> = document
        .get("error-codes")
        .and_then(Value::as_array)
        .map(|codes| codes.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    // A fault of this installation is not a fault of the person: it is told apart first.
    if codes
        .iter()
        .any(|code| matches!(*code, "missing-input-secret" | "invalid-input-secret"))
    {
        return Err(Unreachable::new(
            Why::Secret,
            "Google does not accept the secret key",
        ));
    }
    if codes.contains(&"bad-request") {
        return Err(Unreachable::new(Why::Setup, "Google refused the request"));
    }
    for (code, rejected) in [
        ("missing-input-response", Rejected::Missing),
        ("invalid-input-response", Rejected::Invalid),
        // Google's one word for a token that is too old or was used before.
        ("timeout-or-duplicate", Rejected::Expired),
    ] {
        if codes.contains(&code) {
            return Ok(Facts::refused(rejected));
        }
    }
    Err(Unreachable::new(
        Why::Setup,
        format!(
            "an answer this side does not know: {}",
            codes
                .iter()
                .take(3)
                .map(|code| word(code))
                .collect::<Vec<_>>()
                .join(",")
        ),
    ))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use crate::transport::Scripted;
    use futures::executor::block_on;
    use orch_human_check_contract::{Act, Config, Secret};
    use std::time::Duration;

    fn config() -> Config {
        Config {
            site_key: "6Lc-site-key".to_owned(),
            project_id: String::new(),
            secret: Secret::new("6Lc-the-secret-key"),
            timeout: Duration::from_secs(3),
        }
    }

    fn said(body: &str) -> Result<Facts, Unreachable> {
        read(&Reply {
            status: 200,
            body: body.as_bytes().to_vec(),
        })
    }

    #[test]
    fn the_request_is_a_form_with_the_secret_the_token_and_the_address() {
        let vendor = Arc::new(Scripted::saying(200, r#"{"success":true}"#));
        let config = config();
        let backend = RecaptchaSiteVerify::new(Arc::clone(&vendor) as Arc<dyn Transport>);
        let _ = block_on(backend.verify(&Request {
            token: "the-token",
            act: Act::SignIn,
            remote_ip: Some("203.0.113.9".parse().unwrap()),
            user_agent: Some("a browser"),
            config: &config,
        }));
        let calls = vendor.calls();
        assert_eq!(calls[0].url, VERIFY_URL);
        assert_eq!(calls[0].timeout, Duration::from_secs(3));
        let Body::Form(fields) = &calls[0].body else {
            panic!("a form is sent");
        };
        assert_eq!(
            fields,
            &vec![
                ("secret".to_owned(), "6Lc-the-secret-key".to_owned()),
                ("response".to_owned(), "the-token".to_owned()),
                ("remoteip".to_owned(), "203.0.113.9".to_owned()),
            ]
        );
    }

    #[test]
    fn a_score_based_answer_carries_the_site_the_action_and_the_score() {
        // The shape the console's own key answered with: success, the moment, the site, the score and the action.
        let facts = said(
            r#"{"success":true,"challenge_ts":"2026-11-13T12:00:00Z","hostname":"orch.example","score":0.9,"action":"orch_sign_in"}"#,
        )
        .unwrap();
        assert_eq!(
            facts,
            Facts {
                rejected: None,
                hostname: Some("orch.example".to_owned()),
                action: Some("orch_sign_in".to_owned()),
                score: Some(0.9),
                reasons: Vec::new(),
            }
        );
    }

    #[test]
    fn a_checkbox_answer_carries_no_score_and_no_action_and_is_not_one_this_backend_accepts() {
        let facts = said(
            r#"{"success":true,"challenge_ts":"2026-11-13T12:00:00Z","hostname":"orch.example"}"#,
        )
        .unwrap();
        assert_eq!((facts.score, facts.action.clone()), (None, None));
        let backend = RecaptchaSiteVerify::new(Arc::new(Scripted::default()));
        let verdict = orch_human_check_contract::judge(
            backend.traits(),
            Ok(facts),
            &orch_human_check_contract::Expectation {
                act: Act::SignIn,
                hosts: &[],
                least_score_percent: 50,
            },
        );
        assert_eq!(
            verdict,
            orch_human_check_contract::Verdict::Fail(orch_human_check_contract::Failure::WrongAct)
        );
    }

    #[test]
    fn each_siteverify_error_code_reads_as_one_fact() {
        for (code, rejected) in [
            ("missing-input-response", Rejected::Missing),
            ("invalid-input-response", Rejected::Invalid),
            ("timeout-or-duplicate", Rejected::Expired),
        ] {
            assert_eq!(
                said(&format!(r#"{{"success":false,"error-codes":["{code}"]}}"#)),
                Ok(Facts::refused(rejected)),
                "{code}"
            );
        }
        for (code, why) in [
            ("missing-input-secret", Why::Secret),
            ("invalid-input-secret", Why::Secret),
            ("bad-request", Why::Setup),
            ("a-code-nobody-has-heard-of", Why::Setup),
        ] {
            let error =
                said(&format!(r#"{{"success":false,"error-codes":["{code}"]}}"#)).unwrap_err();
            assert_eq!(error.why, why, "{code}");
        }
    }

    #[test]
    fn an_answer_that_is_not_json_is_the_vendors_fault_by_its_status() {
        let error = read(&Reply {
            status: 502,
            body: b"bad gateway".to_vec(),
        })
        .unwrap_err();
        assert_eq!(error.why, Why::Status);
        assert_eq!(said("<html>").unwrap_err().why, Why::Unreadable);
    }

    #[test]
    fn the_secret_is_in_no_error_and_no_debug_line() {
        let config = config();
        assert!(!format!("{config:?}").contains("the-secret-key"));
        let error =
            said(r#"{"success":false,"error-codes":["invalid-input-secret"]}"#).unwrap_err();
        assert!(!error.to_string().contains("the-secret-key"));
    }

    #[test]
    fn the_siteverify_backend_speaks_for_recaptcha_in_its_own_way() {
        let backend = RecaptchaSiteVerify::new(Arc::new(Scripted::default()));
        assert_eq!(backend.service(), "recaptcha");
        assert_eq!(backend.way(), "siteverify");
        assert!(
            backend
                .edge_hosts(true)
                .script
                .contains(&"https://www.recaptcha.net/recaptcha/")
        );
    }
}
