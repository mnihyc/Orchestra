//! Google Cloud Fraud Defense, the product that reCAPTCHA is part of: the Assessments API (`CreateAssessment`) for a
//! score-based key, the way Google recommends.
//!
//! The page's widget (`enterprise.js`) gives the browser a token made for an action; this backend creates an
//! assessment for it with the Google Cloud project's API key and reads whether the token is valid, which action and
//! site it was made for, and its risk score. The key travels as the `key` query of the address, as Google's guide
//! shows it, so this module never writes an address into an error. The older `siteverify` call is the module beside
//! this one ([`crate::recaptcha_siteverify`]); both speak for the service `recaptcha`.

use std::sync::Arc;

use async_trait::async_trait;
use orch_human_check_contract::{
    ASSESSMENTS, EdgeHosts, Facts, HumanCheckProvider, RECAPTCHA, Rejected, Request, Traits,
    Unreachable, Why,
};
use serde_json::{Value, json};

use crate::transport::{Body, Reply, Transport};
use crate::word;

/// The service that creates assessments.
pub const ASSESSMENTS_HOST: &str = "https://recaptchaenterprise.googleapis.com";

use crate::recaptcha::{EDGE_HOSTS, EDGE_HOSTS_ALTERNATE};

/// Google Cloud Fraud Defense, asked through its Assessments API.
pub struct RecaptchaAssessments {
    transport: Arc<dyn Transport>,
}

impl RecaptchaAssessments {
    /// The backend, speaking through `transport`.
    #[must_use]
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self { transport }
    }
}

/// Whether `id` can be a Google Cloud project: its characters are those of an identifier, so that nothing else can
/// be put into the path of the call.
fn project_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

/// The address of the call, with the key in its query.
fn assessments_url(project: &str, key: &str) -> Result<String, Unreachable> {
    if !project_ok(project) {
        return Err(Unreachable::new(
            Why::Setup,
            "the Google Cloud project is not set, or is not an identifier",
        ));
    }
    let mut url = url::Url::parse(&format!(
        "{ASSESSMENTS_HOST}/v1/projects/{project}/assessments"
    ))
    .map_err(|_| Unreachable::new(Why::Setup, "the Google Cloud project is not an identifier"))?;
    url.query_pairs_mut().append_pair("key", key);
    Ok(url.into())
}

#[async_trait]
impl HumanCheckProvider for RecaptchaAssessments {
    fn kind(&self) -> &'static str {
        "recaptcha-assessments"
    }

    fn service(&self) -> &'static str {
        RECAPTCHA
    }

    fn way(&self) -> &'static str {
        ASSESSMENTS
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
        let url = assessments_url(&request.config.project_id, request.config.secret.expose())?;
        let mut event = json!({
            "token": request.token,
            "siteKey": request.config.site_key,
            "expectedAction": request.act.vendor_action(),
        });
        if let Some(ip) = request.remote_ip {
            event["userIpAddress"] = json!(ip.to_string());
        }
        if let Some(agent) = request.user_agent {
            event["userAgent"] = json!(agent);
        }
        let body = json!({ "event": event }).to_string();
        let reply = self
            .transport
            .post(&url, Body::Json(body), request.config.timeout)
            .await?;
        read(&reply)
    }
}

/// What an assessment says of the token.
pub(crate) fn read(reply: &Reply) -> Result<Facts, Unreachable> {
    let document: Option<Value> = serde_json::from_slice(&reply.body).ok();
    if reply.status != 200 {
        // The words of an error are Google's own status word, never its message, which may quote the request.
        let status_word = document
            .as_ref()
            .and_then(|document| document.pointer("/error/status"))
            .and_then(Value::as_str)
            .map(word)
            .unwrap_or_default();
        let detail = if status_word.is_empty() {
            format!("status {}", reply.status)
        } else {
            format!("status {} {status_word}", reply.status)
        };
        let message = document
            .as_ref()
            .and_then(|document| document.pointer("/error/message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_lowercase();
        let why = match reply.status {
            401 | 403 => Why::Secret,
            400 if message.contains("api key") => Why::Secret,
            400 | 404 => Why::Setup,
            _ => Why::Status,
        };
        return Err(Unreachable::new(why, detail));
    }
    let Some(document) = document else {
        return Err(Unreachable::new(Why::Unreadable, "the answer is not JSON"));
    };
    let Some(valid) = document
        .pointer("/tokenProperties/valid")
        .and_then(Value::as_bool)
    else {
        return Err(Unreachable::new(
            Why::Unreadable,
            "the answer holds no verdict on the token",
        ));
    };
    if !valid {
        // Only the three reasons a person can act on are told apart; any other is a token that is not good.
        let rejected = match document
            .pointer("/tokenProperties/invalidReason")
            .and_then(Value::as_str)
        {
            Some("EXPIRED") => Rejected::Expired,
            Some("DUPLICATE") => Rejected::Replayed,
            Some("MISSING") => Rejected::Missing,
            _ => Rejected::Invalid,
        };
        return Ok(Facts::refused(rejected));
    }
    let text = |pointer: &str| {
        document
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Ok(Facts {
        rejected: None,
        hostname: text("/tokenProperties/hostname"),
        action: text("/tokenProperties/action"),
        score: document
            .pointer("/riskAnalysis/score")
            .and_then(Value::as_f64),
        reasons: document
            .pointer("/riskAnalysis/reasons")
            .and_then(Value::as_array)
            .map(|reasons| reasons.iter().filter_map(Value::as_str).map(word).collect())
            .unwrap_or_default(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use crate::transport::Scripted;
    use futures::executor::block_on;
    use orch_human_check_contract::{Act, Config, Secret};
    use std::time::Duration;

    fn config(project: &str) -> Config {
        Config {
            site_key: "6Lc-site-key".to_owned(),
            project_id: project.to_owned(),
            secret: Secret::new("AIza-the-api-key"),
            timeout: Duration::from_secs(4),
        }
    }

    fn ask(vendor: &Arc<Scripted>, project: &str) -> Result<Facts, Unreachable> {
        let config = config(project);
        let backend = RecaptchaAssessments::new(Arc::clone(vendor) as Arc<dyn Transport>);
        block_on(backend.verify(&Request {
            token: "the-token",
            act: Act::Register,
            remote_ip: Some("2001:db8::7".parse().unwrap()),
            user_agent: Some("Mozilla/5.0"),
            config: &config,
        }))
    }

    fn said(status: u16, body: &str) -> Result<Facts, Unreachable> {
        read(&Reply {
            status,
            body: body.as_bytes().to_vec(),
        })
    }

    const GOOD: &str = r#"{
        "name": "projects/p/assessments/a1",
        "event": {"token": "x", "siteKey": "k", "expectedAction": "orch_register"},
        "riskAnalysis": {"score": 0.9, "reasons": ["LOW_CONFIDENCE_SCORE"]},
        "tokenProperties": {"valid": true, "hostname": "orch.example", "action": "orch_register",
                            "createTime": "2026-11-13T10:00:00Z"}
    }"#;

    #[test]
    fn the_call_is_an_assessment_of_the_token_for_the_act_with_the_key_in_the_query() {
        let vendor = Arc::new(Scripted::saying(200, GOOD));
        let facts = ask(&vendor, "my-project").unwrap();
        assert_eq!(facts.action.as_deref(), Some("orch_register"));
        let calls = vendor.calls();
        assert_eq!(
            calls[0].url,
            "https://recaptchaenterprise.googleapis.com/v1/projects/my-project/assessments?key=AIza-the-api-key"
        );
        assert_eq!(calls[0].timeout, Duration::from_secs(4));
        let Body::Json(text) = &calls[0].body else {
            panic!("the call is JSON");
        };
        let sent: Value = serde_json::from_str(text).unwrap();
        assert_eq!(sent["event"]["token"], "the-token");
        assert_eq!(sent["event"]["siteKey"], "6Lc-site-key");
        assert_eq!(sent["event"]["expectedAction"], "orch_register");
        assert_eq!(sent["event"]["userIpAddress"], "2001:db8::7");
        assert_eq!(sent["event"]["userAgent"], "Mozilla/5.0");
    }

    #[test]
    fn an_assessment_reads_valid_action_hostname_score_and_reasons() {
        let facts = said(200, GOOD).unwrap();
        assert_eq!(
            facts,
            Facts {
                rejected: None,
                hostname: Some("orch.example".to_owned()),
                action: Some("orch_register".to_owned()),
                score: Some(0.9),
                reasons: vec!["LOW_CONFIDENCE_SCORE".to_owned()],
            }
        );
    }

    #[test]
    fn a_token_that_is_not_valid_is_refused_with_the_reason_a_person_can_act_on() {
        for (reason, rejected) in [
            ("EXPIRED", Rejected::Expired),
            ("DUPLICATE", Rejected::Replayed),
            ("MISSING", Rejected::Missing),
            ("MALFORMED", Rejected::Invalid),
            ("SITE_MISMATCH", Rejected::Invalid),
        ] {
            let body =
                format!(r#"{{"tokenProperties":{{"valid":false,"invalidReason":"{reason}"}}}}"#);
            assert_eq!(said(200, &body), Ok(Facts::refused(rejected)), "{reason}");
        }
    }

    #[test]
    fn an_assessment_with_an_unknown_invalid_reason_is_a_failure_not_an_error() {
        let body = r#"{"tokenProperties":{"valid":false,"invalidReason":"A_REASON_NOBODY_HAS_HEARD_OF"},"extra":{"a":1}}"#;
        assert_eq!(said(200, body), Ok(Facts::refused(Rejected::Invalid)));
        let body = r#"{"tokenProperties":{"valid":false}}"#;
        assert_eq!(said(200, body), Ok(Facts::refused(Rejected::Invalid)));
    }

    #[test]
    fn an_answer_with_fields_this_side_does_not_know_is_still_read() {
        let body = r#"{"tokenProperties":{"valid":true,"hostname":"h","action":"a","new":1},
                       "riskAnalysis":{"score":0.1,"reasons":[],"extendedVerdictReasons":["x"]},
                       "accountDefenderAssessment":{}}"#;
        let facts = said(200, body).unwrap();
        assert_eq!(facts.score, Some(0.1));
    }

    #[test]
    fn a_key_or_project_the_vendor_refuses_is_the_installations_fault_and_not_the_persons() {
        for (status, body, why) in [
            (
                403,
                r#"{"error":{"code":403,"message":"The caller does not have permission","status":"PERMISSION_DENIED"}}"#,
                Why::Secret,
            ),
            (401, "{}", Why::Secret),
            (
                400,
                r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT"}}"#,
                Why::Secret,
            ),
            (
                400,
                r#"{"error":{"code":400,"message":"bad site key","status":"INVALID_ARGUMENT"}}"#,
                Why::Setup,
            ),
            (
                404,
                r#"{"error":{"code":404,"status":"NOT_FOUND"}}"#,
                Why::Setup,
            ),
            (
                429,
                r#"{"error":{"status":"RESOURCE_EXHAUSTED"}}"#,
                Why::Status,
            ),
            (503, "", Why::Status),
        ] {
            let error = said(status, body).unwrap_err();
            assert_eq!(error.why, why, "{status} {body}");
            assert!(
                !error.detail.contains("bad site key") && !error.detail.contains("not valid"),
                "{}",
                error.detail
            );
        }
        assert_eq!(
            said(403, r#"{"error":{"status":"PERMISSION_DENIED"}}"#)
                .unwrap_err()
                .detail,
            "status 403 PERMISSION_DENIED"
        );
    }

    #[test]
    fn an_answer_with_no_verdict_cannot_be_used() {
        assert_eq!(said(200, "{}").unwrap_err().why, Why::Unreadable);
        assert_eq!(said(200, "<html>").unwrap_err().why, Why::Unreadable);
    }

    #[test]
    fn a_project_that_is_not_an_identifier_is_not_called() {
        for project in ["", "a/b", "a b", "a?b=c", "../x", "a#b"] {
            let vendor = Arc::new(Scripted::saying(200, GOOD));
            let error = ask(&vendor, project).unwrap_err();
            assert_eq!(error.why, Why::Setup, "{project:?}");
            assert!(vendor.calls().is_empty(), "{project:?}");
        }
    }

    #[test]
    fn the_key_is_in_no_error_and_no_debug_line() {
        let vendor = Arc::new(Scripted::new(vec![Err(Unreachable::new(
            Why::Connect,
            "the vendor could not be reached",
        ))]));
        let error = ask(&vendor, "p").unwrap_err();
        assert!(!error.to_string().contains("AIza"));
        assert!(!format!("{:?}", config("p")).contains("AIza"));
    }

    #[test]
    fn the_key_is_encoded_into_the_query_and_nothing_else_is() {
        let url = assessments_url("p", "a key&x=1").unwrap();
        assert_eq!(
            url,
            "https://recaptchaenterprise.googleapis.com/v1/projects/p/assessments?key=a+key%26x%3D1"
        );
    }

    #[test]
    fn the_assessments_backend_binds_the_act_and_the_site_and_scores_and_has_an_alternate_host() {
        let backend = RecaptchaAssessments::new(Arc::new(Scripted::default()));
        let traits = backend.traits();
        assert!(traits.binds_action && traits.binds_site && traits.scores);
        assert_eq!(backend.service(), "recaptcha");
        assert_eq!(backend.way(), "assessments");
        assert!(
            backend
                .edge_hosts(true)
                .script
                .contains(&"https://www.recaptcha.net/recaptcha/")
        );
        assert!(
            backend
                .edge_hosts(false)
                .script
                .contains(&"https://www.google.com/recaptcha/")
        );
    }
}
