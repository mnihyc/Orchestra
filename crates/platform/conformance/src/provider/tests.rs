//! Controlled broken counterparts prove that each provider rule reports the behavior it names.

use super::*;
use async_trait::async_trait;
use orch_provider_contract::source::{FactsReport, ModelsReport, Presented, ResetReport};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::sync::atomic::{AtomicI64, Ordering};

const CANARY: &str = "provider-refusal-secret-canary";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fault {
    None,
    FreshResult,
    Descriptor,
    Shape,
    States,
    Models,
    Reset,
    Present,
    Quiet,
    Bounds,
    Huge,
    Stall,
}

struct Counterpart {
    fault: Fault,
    spent: AtomicI64,
    reset_replies: AtomicI64,
}

fn decoded<T: DeserializeOwned>(value: Json) -> Result<T, SourceError> {
    serde_json::from_value(value)
        .map_err(|_| error("malformed", "the controlled source returned another shape"))
}

fn error(code: &str, message: &str) -> SourceError {
    SourceError::new(code.parse().unwrap(), message)
}

fn mode(ask: &impl Serialize) -> String {
    serde_json::to_value(ask)
        .unwrap()
        .pointer("/config/mode")
        .and_then(Json::as_str)
        .unwrap_or("normal")
        .to_owned()
}

fn descriptor(fault: Fault) -> SourceDescriptor {
    serde_json::from_value(json!({"name":"controlled","title":if fault==Fault::Descriptor {""} else {"Controlled source"},
        "summary":"A disposable suite counterpart.","contract":1,"facts":{"reports":["windows","balances","available_resets"],
            "spends_resets":true,"credential":"none","min_interval_secs":60,"plans":{}},
        "models":{},"presents":{},"fields":[],"docs_url":null})).unwrap()
}

fn told(count: i64) -> Json {
    json!({"windows":[{"id":"five","label":"5 hours","duration_minutes":300,"used_percent":25.0,
    "resets_at":"2026-10-15T05:00:00.000Z"}],"balances":[{"id":"usd","unit":"USD","remaining":"12.50"}],
    "available_resets":{"available_count":count,"credits":[{"id":"one","reset_type":"window",
        "status":if count==2 {"available"} else {"redeemed"},"granted_at":null,"expires_at":null}]}})
}

#[async_trait]
impl FactsSource for Counterpart {
    fn describe(&self) -> SourceDescriptor {
        descriptor(self.fault)
    }
    async fn read_facts(&self, ask: SourceAsk) -> Result<FactsReport, SourceError> {
        let mode = mode(&ask);
        if self.fault == Fault::Stall {
            return std::future::pending().await;
        }
        if self.fault == Fault::Huge {
            return Ok(FactsReport(json!({"x_padding":"x".repeat(REPLY_BYTES+1)})));
        }
        if mode == "refused" {
            return Err(error(
                "refused",
                if self.fault == Fault::Quiet {
                    CANARY
                } else {
                    "the controlled refusal"
                },
            ));
        }
        if self.fault == Fault::Bounds {
            return Err(error(
                "too_large",
                "the controlled reply exceeds its byte bound",
            ));
        }
        let mut value = match mode.as_str() {
            "absent" if self.fault != Fault::States => json!({}),
            "absent" | "empty" => json!({"windows":[],"balances":[],"available_resets":null}),
            "null" => json!({"windows":null,"balances":null,"available_resets":null}),
            _ => told(2 - self.spent.load(Ordering::SeqCst)),
        };
        if self.fault == Fault::Shape && mode == "normal" {
            value["balances"][0]["remaining"] = json!(12.5);
        }
        decoded(value)
    }
    async fn spend_reset(
        &self,
        _ask: SourceAsk,
        _reset: ResetRequest,
    ) -> Result<ResetReport, SourceError> {
        let spent = if self.fault == Fault::Reset {
            self.spent.fetch_add(1, Ordering::SeqCst) + 1
        } else {
            self.spent.store(1, Ordering::SeqCst);
            1
        };
        let reply = self.reset_replies.fetch_add(1, Ordering::SeqCst);
        let outcome = if self.fault == Fault::FreshResult {
            format!("The reset is spent; fresh read {reply}.")
        } else {
            "One controlled reset.".into()
        };
        decoded(json!({"outcome":outcome,"facts":told(2-spent)}))
    }
}

#[async_trait]
impl ModelsSource for Counterpart {
    fn describe(&self) -> SourceDescriptor {
        descriptor(self.fault)
    }
    async fn list_models(&self, ask: SourceAsk) -> Result<ModelsReport, SourceError> {
        match mode(&ask).as_str() {
            "refused" => Err(error(
                "refused",
                if self.fault == Fault::Quiet {
                    CANARY
                } else {
                    "the controlled refusal"
                },
            )),
            "null" if self.fault != Fault::Models => decoded(json!({"models":null})),
            "null" | "empty" => decoded(json!({"models":[]})),
            _ => decoded(
                json!({"models":[{"model_name":"example","display_name":"Example","parameters":{},"listed":true}]}),
            ),
        }
    }
}

#[async_trait]
impl CredentialPresenter for Counterpart {
    fn describe(&self) -> SourceDescriptor {
        descriptor(self.fault)
    }
    async fn present(&self, ask: PresentAsk) -> Result<Presented, SourceError> {
        if serde_json::to_value(ask)
            .unwrap()
            .pointer("/credential/key")
            == Some(&json!("refuse-key"))
        {
            return Err(error(
                "refused",
                if self.fault == Fault::Quiet {
                    CANARY
                } else {
                    "the controlled refusal"
                },
            ));
        }
        decoded(
            json!({"headers":{"authorization":if self.fault==Fault::Present {"Bearer bad\r\nInjected: header"} else {"Bearer issued-example"}},
            "expires_at":"2026-10-15T00:05:00.000Z"}),
        )
    }
}

fn ask(mode: &str) -> SourceAsk {
    serde_json::from_value(
        json!({"provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Disposable"},
    "endpoints":[],"config":{"mode":mode},"secrets":{"canary":CANARY},"credential":null}),
    )
    .unwrap()
}

fn present_ask(key: &str) -> PresentAsk {
    serde_json::from_value(json!({"credential":{"key":key},"secrets":{"canary":CANARY},
    "provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Disposable"},
    "endpoint":{"protocol":"openai-responses","base_url":"https://example.invalid/v1"}}))
    .unwrap()
}

fn fixture() -> Fixture {
    Fixture {
        facts: vec![
            Example {
                ask: ask("normal"),
                expected: told(2),
            },
            Example {
                ask: ask("absent"),
                expected: json!({}),
            },
            Example {
                ask: ask("null"),
                expected: json!({"windows":null,"balances":null,"available_resets":null}),
            },
            Example {
                ask: ask("empty"),
                expected: json!({"windows":[],"balances":[],"available_resets":null}),
            },
        ],
        models: vec![
            Example {
                ask: ask("null"),
                expected: json!({"models":null}),
            },
            Example {
                ask: ask("empty"),
                expected: json!({"models":[]}),
            },
            Example {
                ask: ask("normal"),
                expected: json!({"models":[{"model_name":"example","display_name":"Example","parameters":{},"listed":true}]}),
            },
        ],
        reset: Some(ResetExample {
            ask: ask("normal"),
            reset: serde_json::from_value(json!({"reset_id":"one","idempotency_key":"replay-one"}))
                .unwrap(),
        }),
        present: Some(PresentExample {
            ask: present_ask("example-key"),
            expected: None,
        }),
        refusal: Some(RefusalExample {
            ask: ask("refused"),
            present: Some(present_ask("refuse-key")),
        }),
        now: "2026-10-15T00:00:00.000Z".parse().unwrap(),
        secrets: vec![CANARY.into(), "example-key".into()],
    }
}

async fn exercise(fault: Fault, fixture: Option<&Fixture>) -> Report {
    let source = Counterpart {
        fault,
        spent: AtomicI64::new(0),
        reset_replies: AtomicI64::new(0),
    };
    let descriptor = descriptor(fault);
    let mut report = Report::new("controlled provider");
    run(
        Sources {
            descriptor: &descriptor,
            facts: Some(&source),
            models: Some(&source),
            presenter: Some(&source),
        },
        fixture,
        &mut report,
    )
    .await;
    report
}

#[tokio::test]
async fn every_rule_passes_a_controlled_source_and_each_deliberate_fault_fails_by_its_name() {
    for valid in [Fault::None, Fault::FreshResult] {
        let report = exercise(valid, Some(&fixture())).await;
        assert!(report.passed(), "{report}");
        assert_eq!(
            report.counts(),
            (8, 0, 0),
            "every declared element was exercised"
        );
    }
    for (fault, rule) in [
        (Fault::Descriptor, "provider.descriptor"),
        (Fault::Shape, "provider.facts.shape"),
        (Fault::States, "provider.facts.states"),
        (Fault::Models, "provider.models"),
        (Fault::Reset, "provider.resets.idempotent"),
        (Fault::Present, "provider.credential.present"),
        (Fault::Quiet, "provider.quiet"),
        (Fault::Bounds, "provider.bounds"),
        (Fault::Huge, "provider.bounds"),
    ] {
        let report = exercise(fault, Some(&fixture())).await;
        assert!(
            matches!(
                report.outcome(rule),
                Some(crate::report::Outcome::Failed(_))
            ),
            "{rule}: {report}"
        );
        assert!(
            !report.to_string().contains(CANARY),
            "even a failing quiet case must not print the leaked value"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_source_that_never_answers_fails_its_deadline_with_virtual_time_and_no_wall_clock_speed_claim()
 {
    let mut controlled = fixture();
    controlled.facts.truncate(1);
    controlled.models.clear();
    controlled.reset = None;
    controlled.present = None;
    controlled.refusal = None;
    let report = exercise(Fault::Stall, Some(&controlled)).await;
    assert!(
        matches!(report.outcome("provider.bounds"),Some(crate::report::Outcome::Failed(why)) if why.contains("exceeded its deadline")),
        "{report}"
    );
}

#[tokio::test]
async fn a_missing_disposable_fixture_verifies_no_reads_exchanges_or_resets() {
    let report = exercise(Fault::None, None).await;
    assert_eq!(report.counts(), (1, 0, 7));
    assert!(matches!(
        report.outcome("provider.resets.idempotent"),
        Some(crate::report::Outcome::Skipped(_))
    ));
}
