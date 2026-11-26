//! hCaptcha: `https://api.hcaptcha.com/siteverify`.
//!
//! The request is form-encoded with the secret, the token, the person's address, and the site key (which stops a token
//! made for another key being redeemed here). The answer says `success`, and on failure a list of error words. hCaptcha
//! itself says that the hostname of its answer comes from the person's browser and is not to be believed, and its
//! tokens carry no act: this backend reports neither as something the decision may ask for. The risk score of its
//! Enterprise plan is not read: its range and direction are not documented where this was written.

use std::sync::Arc;

use async_trait::async_trait;
use orch_human_check_contract::{
    EdgeHosts, Facts, HCAPTCHA, HumanCheckProvider, Rejected, Request, Traits, Unreachable, Why,
};
use serde_json::Value;

use crate::transport::{Body, Reply, Transport};
use crate::word;

/// Where a token is verified.
pub const VERIFY_URL: &str = "https://api.hcaptcha.com/siteverify";

/// The hosts hCaptcha asks a Content-Security-Policy to allow, in the four directives it names. It asks that
/// no subdomain be written out.
pub const EDGE_HOSTS: EdgeHosts = EdgeHosts {
    script: &["https://hcaptcha.com", "https://*.hcaptcha.com"],
    frame: &["https://hcaptcha.com", "https://*.hcaptcha.com"],
    connect: &["https://hcaptcha.com", "https://*.hcaptcha.com"],
    style: &["https://hcaptcha.com", "https://*.hcaptcha.com"],
};

/// hCaptcha.
pub struct HCaptcha {
    transport: Arc<dyn Transport>,
}

impl HCaptcha {
    /// The backend, speaking through `transport`.
    #[must_use]
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport }
    }
}

#[async_trait]
impl HumanCheckProvider for HCaptcha {
    fn kind(&self) -> &'static str {
        HCAPTCHA
    }

    fn service(&self) -> &'static str {
        HCAPTCHA
    }

    fn traits(&self) -> Traits {
        Traits {
            binds_action: false,
            binds_site: false,
            scores: false,
        }
    }

    fn edge_hosts(&self, _alternate: bool) -> EdgeHosts {
        EDGE_HOSTS
    }

    async fn verify(&self, request: &Request<'_>) -> Result<Facts, Unreachable> {
        let mut form = vec![
            (
                "secret".to_owned(),
                request.config.secret.expose().to_owned(),
            ),
            ("response".to_owned(), request.token.to_owned()),
        ];
        if !request.config.site_key.is_empty() {
            form.push(("sitekey".to_owned(), request.config.site_key.clone()));
        }
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

/// What hCaptcha's answer says of the token.
pub(crate) fn read(reply: &Reply) -> Result<Facts, Unreachable> {
    let Ok(document) = serde_json::from_slice::<Value>(&reply.body) else {
        return Err(if (200..300).contains(&reply.status) {
            Unreachable::new(Why::Unreadable, "the answer is not JSON")
        } else {
            Unreachable::new(Why::Status, format!("status {}", reply.status))
        });
    };
    if document.get("success").and_then(Value::as_bool) == Some(true) {
        return Ok(Facts {
            hostname: document
                .get("hostname")
                .and_then(Value::as_str)
                .map(str::to_owned),
            ..Facts::accepted()
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
            "hCaptcha does not accept the secret",
        ));
    }
    if codes.iter().any(|code| {
        matches!(
            *code,
            "bad-request" | "missing-remoteip" | "invalid-remoteip"
        )
    }) {
        return Err(Unreachable::new(
            Why::Setup,
            format!("hCaptcha refused the request: {}", word(codes[0])),
        ));
    }
    for (code, rejected) in [
        ("missing-input-response", Rejected::Missing),
        ("invalid-input-response", Rejected::Invalid),
        ("expired-input-response", Rejected::Expired),
        ("already-seen-response", Rejected::Replayed),
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
            site_key: "10000000-ffff-ffff-ffff-000000000001".to_owned(),
            project_id: String::new(),
            secret: Secret::new("0x0000000000000000000000000000000000000000"),
            timeout: Duration::from_secs(5),
        }
    }

    fn ask(vendor: &Arc<Scripted>, token: &str) -> Result<Facts, Unreachable> {
        let config = config();
        let backend = HCaptcha::new(Arc::clone(vendor) as Arc<dyn Transport>);
        block_on(backend.verify(&Request {
            token,
            act: Act::SignIn,
            remote_ip: Some("203.0.113.9".parse().unwrap()),
            user_agent: Some("a browser"),
            config: &config,
        }))
    }

    fn said(body: &str) -> Result<Facts, Unreachable> {
        read(&Reply {
            status: 200,
            body: body.as_bytes().to_vec(),
        })
    }

    #[test]
    fn the_request_is_a_form_with_the_secret_the_token_the_site_key_and_the_address() {
        let vendor = Arc::new(Scripted::saying(200, r#"{"success":true}"#));
        assert_eq!(ask(&vendor, "token-1"), Ok(Facts::accepted()));
        let calls = vendor.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].url, VERIFY_URL);
        assert_eq!(calls[0].timeout, Duration::from_secs(5));
        let Body::Form(fields) = &calls[0].body else {
            panic!("a form is sent: hCaptcha refuses JSON");
        };
        let field = |name: &str| {
            fields
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        };
        assert_eq!(
            field("secret"),
            Some("0x0000000000000000000000000000000000000000")
        );
        assert_eq!(field("response"), Some("token-1"));
        assert_eq!(
            field("sitekey"),
            Some("10000000-ffff-ffff-ffff-000000000001")
        );
        assert_eq!(field("remoteip"), Some("203.0.113.9"));
    }

    #[test]
    fn a_success_carries_the_hostname_for_the_record_and_nothing_else() {
        assert_eq!(
            said(
                r#"{"success":true,"challenge_ts":"2026-11-13T10:00:00Z","hostname":"orch.example","credit":false}"#
            ),
            Ok(Facts {
                hostname: Some("orch.example".to_owned()),
                ..Facts::accepted()
            })
        );
        // An Enterprise answer's score is not read.
        let facts = said(r#"{"success":true,"score":0.9,"score_reason":["x"]}"#).unwrap();
        assert_eq!(facts.score, None);
    }

    #[test]
    fn each_hcaptcha_error_code_reads_as_one_fact() {
        for (code, rejected) in [
            ("missing-input-response", Rejected::Missing),
            ("invalid-input-response", Rejected::Invalid),
            ("expired-input-response", Rejected::Expired),
            ("already-seen-response", Rejected::Replayed),
        ] {
            assert_eq!(
                said(&format!(r#"{{"success":false,"error-codes":["{code}"]}}"#)),
                Ok(Facts::refused(rejected)),
                "{code}"
            );
        }
        for code in ["missing-input-secret", "invalid-input-secret"] {
            let error =
                said(&format!(r#"{{"success":false,"error-codes":["{code}"]}}"#)).unwrap_err();
            assert_eq!(error.why, Why::Secret, "{code}");
        }
        for code in ["bad-request", "missing-remoteip", "invalid-remoteip"] {
            let error =
                said(&format!(r#"{{"success":false,"error-codes":["{code}"]}}"#)).unwrap_err();
            assert_eq!(error.why, Why::Setup, "{code}");
        }
    }

    #[test]
    fn a_code_this_side_does_not_know_is_an_answer_it_cannot_use_and_not_a_failure_of_the_person() {
        let error =
            said(r#"{"success":false,"error-codes":["sitekey-secret-mismatch"]}"#).unwrap_err();
        assert_eq!(error.why, Why::Setup);
        assert!(error.detail.contains("sitekey-secret-mismatch"));
        let error = said(r#"{"success":false}"#).unwrap_err();
        assert_eq!(error.why, Why::Setup);
    }

    #[test]
    fn a_hostname_of_not_provided_is_not_a_failure() {
        let facts = said(r#"{"success":true,"hostname":"not-provided"}"#).unwrap();
        assert_eq!(facts.rejected, None);
    }

    #[test]
    fn an_answer_that_is_not_json_is_the_vendors_fault_by_its_status() {
        let error = read(&Reply {
            status: 503,
            body: b"<html>down</html>".to_vec(),
        })
        .unwrap_err();
        assert_eq!(error.why, Why::Status);
        let error = read(&Reply {
            status: 200,
            body: b"<html>".to_vec(),
        })
        .unwrap_err();
        assert_eq!(error.why, Why::Unreadable);
    }

    #[test]
    fn a_vendor_that_cannot_be_reached_comes_back_as_it_was() {
        let vendor = Arc::new(Scripted::new(vec![Err(Unreachable::new(
            Why::Timeout,
            "slow",
        ))]));
        assert_eq!(
            ask(&vendor, "token").unwrap_err(),
            Unreachable::new(Why::Timeout, "slow")
        );
    }

    #[test]
    fn hcaptcha_binds_neither_an_act_nor_a_site_and_gives_no_score() {
        let backend = HCaptcha::new(Arc::new(Scripted::default()));
        let traits = backend.traits();
        assert!(!traits.binds_action && !traits.binds_site && !traits.scores);
        assert_eq!(backend.service(), "hcaptcha");
        assert_eq!(backend.way(), "");
        assert!(
            backend
                .edge_hosts(false)
                .frame
                .contains(&"https://*.hcaptcha.com")
        );
    }
}
