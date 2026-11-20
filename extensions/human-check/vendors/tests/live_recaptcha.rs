//! A live check of the `siteverify` backend against Google, with a key registered for testing.
//!
//! It runs only where the environment holds the key: `RECAPTCHA_TEST_SITE_KEY`, `RECAPTCHA_TEST_SECRET` and
//! `RECAPTCHA_TEST_DOMAIN`, and the file named by `RECAPTCHA_TEST_TOKEN_FILE`, into which a browser that ran the page
//! under that domain writes the token its widget gave. The case waits for the file, uses the token once, and deletes
//! the file; a second use of the same token is then refused. Nothing it reads is ever written into a message, and
//! without these it says why it was skipped.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use orch_human_check::recaptcha_siteverify::RecaptchaSiteVerify;
use orch_human_check::{Http, Transport};
use orch_human_check_contract::{
    Act, Config, Expectation, Failure, HumanCheckProvider, Request, Secret, Verdict, judge,
};

fn held(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

#[tokio::test]
async fn a_token_the_widget_gave_for_the_registered_domain_passes_once_and_is_refused_the_second_time()
 {
    let (Some(site), Some(secret), Some(domain), Some(file)) = (
        held("RECAPTCHA_TEST_SITE_KEY"),
        held("RECAPTCHA_TEST_SECRET"),
        held("RECAPTCHA_TEST_DOMAIN"),
        held("RECAPTCHA_TEST_TOKEN_FILE"),
    ) else {
        eprintln!("skipped: the key registered for testing is not in the environment");
        return;
    };
    // The token is made by a browser after this case has started, and is good for two minutes: wait for it.
    let began = Instant::now();
    let token = loop {
        if let Ok(text) = std::fs::read_to_string(&file) {
            let token = text.trim().to_owned();
            if !token.is_empty() {
                std::fs::remove_file(&file).unwrap();
                break token;
            }
        }
        assert!(
            began.elapsed() < Duration::from_secs(60 * 40),
            "no token was given within forty minutes"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let transport: Arc<dyn Transport> = Arc::new(Http::new().unwrap());
    let backend = RecaptchaSiteVerify::new(transport);
    let config = Config {
        site_key: site,
        project_id: String::new(),
        secret: Secret::new(secret),
        timeout: Duration::from_secs(10),
    };
    let hosts = vec![domain.to_lowercase()];
    let mut verdicts = Vec::new();
    for _ in 0..2 {
        let answer = backend
            .verify(&Request {
                token: &token,
                act: Act::SignIn,
                remote_ip: None,
                user_agent: None,
                config: &config,
            })
            .await;
        if let Ok(facts) = &answer {
            // What the vendor carried, as booleans and the score alone.
            eprintln!(
                "answer carried: rejected={:?} hostname_is_the_domain={} action={:?} score_present={}",
                facts.rejected,
                facts.hostname.as_deref().map(str::to_lowercase) == hosts.first().cloned(),
                facts.action,
                facts.score.is_some()
            );
        }
        verdicts.push(judge(
            backend.traits(),
            answer,
            &Expectation {
                act: Act::SignIn,
                hosts: &hosts,
                least_score_percent: 50,
            },
        ));
    }
    assert_eq!(verdicts[0], Verdict::Pass, "the live token did not pass");
    assert_eq!(
        verdicts[1],
        Verdict::Fail(Failure::Expired),
        "Google gives one word for a token that is old or used before"
    );
}
