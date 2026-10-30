//! The summarizer, notifier, driver, executor and translator bridges: each trait's methods, round-tripped through
//! the example plugins or the scripted one, and the set of plugins a component holds.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use orch_driver_contract::driver::{DriverError, EnvironmentDriver};
use orch_driver_contract::imports::ImportRequest;
use orch_driver_contract::runtime::{RuntimeKind, RuntimeSpec};
use orch_driver_contract::units::{UnitRef, UnitStateKind};
use orch_driver_contract::volumes::VolumeRequest;
use orch_executor_contract::context::EnvironmentContext;
use orch_executor_contract::executor::Executor;
use orch_model_contract::credentials::Credential;
use orch_model_contract::translator::{ProtocolTranslator, TranslateError, UpstreamSide};
use orch_notifier_contract::delivery::{Config, DeliverError, Delivery};
use orch_notifier_contract::notifier::Notifier;
use orch_plugin::bridges::driver::DriverBridge;
use orch_plugin::bridges::executor::{ExecutorBridge, ExecutorFactory};
use orch_plugin::bridges::notifier::NotifierBridge;
use orch_plugin::bridges::summarizer::SummarizerBridge;
use orch_plugin::bridges::translator::TranslatorBridge;
use orch_plugin::registration::{BuiltIn, LoadError, Plugins};
use orch_plugin::supervisor::Plugin;
use orch_plugin::{BridgeError, hosted_by};
use orch_plugin_contract::registration::PluginHost;
use orch_plugin_contract::{PluginKind, PluginRegistration, PluginState};
use orch_summarizer_contract::descriptor::SummaryField;
use orch_summarizer_contract::summarizer::{SummarizeError, Summarizer, SummaryRequest};
use orch_types::{Json, ManualClock};
use serde_json::json;
use support::{
    PYTHON, Work, advance, clock, example, options, registration, script, settle, wait_until,
};

const THREAD: &str = "0199abcd-1234-7000-8000-0000000000a1";
const RUNTIME: &str = "01990000-0000-7000-8000-0199abcd1234";

async fn start(entry: &PluginRegistration, clock: &ManualClock) -> Plugin {
    let plugin = Plugin::start(entry, options(clock)).await.unwrap();
    plugin.settled().await;
    assert_eq!(
        plugin.report().state,
        PluginState::Running,
        "{:?}",
        plugin.report().detail
    );
    plugin
}

fn example_entry(
    kind: PluginKind,
    name: &str,
    directory: &str,
    file: &str,
    args: &[String],
) -> PluginRegistration {
    let mut command = vec![PYTHON.to_owned(), example(directory, file)];
    command.extend(args.iter().cloned());
    PluginRegistration {
        kind,
        name: name.to_owned(),
        command: Some(command),
        connect: None,
        env: BTreeMap::new(),
        restart: None,
    }
}

// ---- summarizer -------------------------------------------------------------------------------------------

fn summary_request(field: &str, activity: &Json, config: &Json) -> SummaryRequest {
    serde_json::from_value(json!({
        "field": field,
        "node": { "id": THREAD, "kind": "thread", "name": "Fix login", "redacted": false },
        "previous": "", "activity": activity, "tag_choices": ["login", "docs", "infra"],
        "limits": { "max_chars": if field == "title" { 80 } else { 200 }, "max_tags": 2 },
        "config": config
    }))
    .unwrap()
}

#[tokio::test]
async fn the_example_summarizer_answers_each_field_from_its_rule_through_the_summarizer_trait() {
    let clock = clock();
    let plugin = start(
        &example_entry(
            PluginKind::Summarizer,
            "rules",
            "summarizer",
            "summarizer.py",
            &[],
        ),
        &clock,
    )
    .await;
    let summarizer = SummarizerBridge::new(plugin.clone()).unwrap();
    let described = summarizer.describe();
    assert_eq!(described.name, "rules");
    assert_eq!(
        described.fields,
        [
            SummaryField::Title,
            SummaryField::Description,
            SummaryField::Tags
        ]
    );
    assert_eq!(described.check(), Ok(()));
    assert!(
        described.extensions.get("x_rules").is_some(),
        "members of its own are kept"
    );

    let activity = json!([
        { "kind": "user_message", "text": "Fix the login redirect\nit loops" },
        { "kind": "agent_message", "text": "Reading the handler\nthen the tests" }
    ]);
    let described = summarizer
        .summarize(summary_request(
            "description",
            &activity,
            &json!({ "prefix": "Now: " }),
        ))
        .await
        .unwrap();
    assert_eq!(described.text.as_deref(), Some("Now: Reading the handler"));
    let titled = summarizer
        .summarize(summary_request("title", &activity, &json!({})))
        .await
        .unwrap();
    assert_eq!(titled.text.as_deref(), Some("Fix the login redirect"));
    let tagged = summarizer
        .summarize(summary_request("tags", &activity, &json!({})))
        .await
        .unwrap();
    assert_eq!(tagged.tags, Some(vec!["login".to_owned()]));
    assert_eq!(
        tagged.usage, None,
        "a summarizer without a model reports no usage"
    );

    // Nothing to summarize is skipped, which is no failure.
    let skipped = summarizer
        .summarize(summary_request("description", &json!([]), &json!({})))
        .await
        .unwrap_err();
    assert!(matches!(skipped, SummarizeError::Skipped(_)));
    assert!(!skipped.is_failure());
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_field_the_summarizer_does_not_list_is_refused_without_asking_it_and_its_errors_keep_their_meaning()
 {
    let (clock, work) = (clock(), Work::new());
    let mut only_descriptions = script(
        PluginKind::Summarizer,
        &["describe", "summarize"],
        &json!({ "summarize": [
            { "result": { "text": "x", "tags": null, "usage": null } },
            { "error": { "code": "config", "message": "no model is set", "retryable": false } },
            { "error": { "code": "overloaded", "message": "try later", "retryable": true } }
        ] }),
    );
    only_descriptions["descriptor"]["fields"] = json!(["description"]);
    let plugin = start(
        &registration(
            PluginKind::Summarizer,
            "rules",
            &work,
            &only_descriptions,
            &[],
        ),
        &clock,
    )
    .await;
    let summarizer = SummarizerBridge::new(plugin.clone()).unwrap();
    let refused = summarizer
        .summarize(summary_request("title", &json!([]), &json!({})))
        .await
        .unwrap_err();
    assert_eq!(refused, SummarizeError::Unsupported(SummaryField::Title));
    assert!(
        work.received("summarize").is_empty(),
        "the plugin was not asked"
    );

    let ask = || summarizer.summarize(summary_request("description", &json!([]), &json!({})));
    assert!(ask().await.is_ok());
    assert_eq!(
        ask().await.unwrap_err(),
        SummarizeError::Config("no model is set".into())
    );
    assert_eq!(
        ask().await.unwrap_err(),
        SummarizeError::Failed {
            message: "try later".into(),
            retryable: true
        }
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_nested_plugin_configuration_is_checked_before_any_request_reaches_the_program() {
    let (clock, work) = (clock(), Work::new());
    let mut declared = script(
        PluginKind::Summarizer,
        &["describe", "summarize"],
        &json!({"summarize":[{"result":{"text":"Accepted","tags":null,"usage":null}}]}),
    );
    declared["descriptor"]["config_schema"] = json!({"type":"object","properties":{"connection":{"type":"object","properties":{"key":{"type":"string"}},"required":["key"],"additionalProperties":false}},"required":["connection"],"additionalProperties":false});
    let plugin = start(
        &registration(PluginKind::Summarizer, "rules", &work, &declared, &[]),
        &clock,
    )
    .await;
    let summarizer = SummarizerBridge::new(plugin.clone()).unwrap();
    let refused = summarizer
        .summarize(summary_request(
            "description",
            &json!([]),
            &json!({"connection":{"key":false,"unknown":"do-not-echo"}}),
        ))
        .await
        .unwrap_err();
    assert!(matches!(refused, SummarizeError::Config(_)));
    assert!(!refused.to_string().contains("do-not-echo"));
    assert!(work.received("summarize").is_empty());
    let accepted = summarizer
        .summarize(summary_request(
            "description",
            &json!([]),
            &json!({"connection":{"key":"okay"}}),
        ))
        .await
        .unwrap();
    assert_eq!(accepted.text.as_deref(), Some("Accepted"));
    assert_eq!(work.received("summarize").len(), 1);
    plugin.shutdown().await;
}

// ---- notifier ---------------------------------------------------------------------------------------------

fn delivery(id: &str, attempt: u32, label: Option<&str>, body: &str) -> Delivery {
    serde_json::from_value(json!({
        "id": id, "attempt": attempt,
        "config": { "values": label.map_or(json!({}), |label| json!({ "label": label })), "secrets": {} },
        "to": {}, "recipient": null, "language": "en", "subject": null, "body": body, "notifications": []
    }))
    .unwrap()
}

#[tokio::test]
async fn the_example_notifier_appends_what_it_is_asked_to_deliver_through_the_notifier_trait() {
    let (clock, work) = (clock(), Work::new());
    let file = work.text("notices.log");
    let plugin = start(
        &example_entry(
            PluginKind::Notifier,
            "file",
            "notifier",
            "notifier.py",
            &["--file".into(), file.clone()],
        ),
        &clock,
    )
    .await;
    let notifier = NotifierBridge::new(plugin.clone()).unwrap();
    let described = notifier.describe();
    assert_eq!(
        (described.name.as_str(), described.max_chars),
        ("file", Some(2000))
    );
    assert_eq!(described.check(), Ok(()));
    assert_eq!(plugin.report().unavailable, ["link_url", "poll_links"]);

    assert_eq!(
        notifier.check(&Config::default()).await.unwrap(),
        json!({ "file": file })
    );
    let first = "0199abcd-1234-7000-8000-0000000000d1";
    notifier
        .deliver(delivery(first, 1, Some("ops"), "Turn finished\nin Web"))
        .await
        .unwrap();
    notifier
        .deliver(delivery(
            "0199abcd-1234-7000-8000-0000000000d2",
            1,
            None,
            "Second",
        ))
        .await
        .unwrap();
    // The same delivery tried again is not written twice.
    notifier
        .deliver(delivery(first, 2, Some("ops"), "Turn finished\nin Web"))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "ops: Turn finished / in Web\nSecond\n"
    );

    // A notifier that links nobody has no link, and says that it cannot be asked to find who followed one.
    assert_eq!(notifier.link_url(&Config::default(), "code"), None);
    let asked = notifier
        .poll_links(&Config::default(), None, Duration::from_secs(25))
        .await
        .unwrap_err();
    assert_eq!(asked.code, "unsupported");
    plugin.shutdown().await;
}

#[tokio::test]
async fn the_example_notifier_refuses_a_file_it_cannot_write_and_a_delivery_that_fails_may_be_tried_again()
 {
    let (clock, work) = (clock(), Work::new());
    let file = work.text("missing-directory/notices.log");
    let plugin = start(
        &example_entry(
            PluginKind::Notifier,
            "file",
            "notifier",
            "notifier.py",
            &["--file".into(), file],
        ),
        &clock,
    )
    .await;
    let notifier = NotifierBridge::new(plugin.clone()).unwrap();
    let refused = notifier.check(&Config::default()).await.unwrap_err();
    assert_eq!(
        (refused.code.as_str(), refused.retryable),
        ("unwritable", false),
        "the save is refused"
    );
    let failed = notifier
        .deliver(delivery(
            "0199abcd-1234-7000-8000-0000000000d1",
            1,
            None,
            "x",
        ))
        .await
        .unwrap_err();
    assert_eq!(
        (failed.code.as_str(), failed.retryable, failed.gone),
        ("write_failed", true, false)
    );
    plugin.shutdown().await;
}

fn notifier_script(responses: &Json) -> Json {
    script(
        PluginKind::Notifier,
        &["describe", "check", "deliver", "link_url", "poll_links"],
        responses,
    )
}

#[tokio::test]
async fn a_notifier_says_that_a_target_is_gone_or_when_to_try_again_and_the_worker_hears_it() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &registration(
            PluginKind::Notifier,
            "chat",
            &work,
            &notifier_script(&json!({ "deliver": [
                { "error": { "code": "blocked", "message": "the bot was stopped", "retryable": false, "gone": true } },
                { "error": { "code": "rate_limited", "message": "slow down", "retryable": true, "retry_after_secs": 30 } }
            ] })),
            &[],
        ),
        &clock,
    )
    .await;
    let notifier = NotifierBridge::new(plugin.clone()).unwrap();
    let send = || {
        notifier.deliver(delivery(
            "0199abcd-1234-7000-8000-0000000000d1",
            1,
            None,
            "x",
        ))
    };
    let gone: DeliverError = send().await.unwrap_err();
    assert!(gone.gone && !gone.retryable);
    let slow = send().await.unwrap_err();
    assert_eq!((slow.retryable, slow.retry_after_secs), (true, Some(30)));
    assert_eq!(
        orch_notifier_contract::delivery::after_delivery(
            &Err(slow),
            1,
            &orch_notifier_contract::delivery::DEFAULT_RETRY_SCHEDULE
        ),
        orch_notifier_contract::delivery::Next::RetryAfter(Duration::from_secs(30)),
        "the far side's time replaces the schedule's"
    );
    plugin.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_notifier_that_links_people_makes_a_link_and_is_asked_who_followed_it() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &registration(
            PluginKind::Notifier,
            "chat",
            &work,
            &notifier_script(&json!({
                "link_url": { "result": { "url": "https://chat.example/start?code=c0de" } },
                "poll_links": { "result": {
                    "started": [ { "code": "c0de", "address": { "chat_id": 42 }, "label": "Dana" } ],
                    "cursor": "7"
                } }
            })),
            &[],
        ),
        &clock,
    )
    .await;
    let notifier = NotifierBridge::new(plugin.clone()).unwrap();
    let config = Config::default();
    // The trait asks for the link without waiting, so the bridge waits for it on the calling thread.
    assert_eq!(
        notifier.link_url(&config, "c0de").as_deref(),
        Some("https://chat.example/start?code=c0de")
    );
    assert_eq!(work.received("link_url")[0]["params"]["code"], "c0de");

    let polled = notifier
        .poll_links(&config, Some("6".into()), Duration::from_secs(25))
        .await
        .unwrap();
    assert_eq!(polled.cursor.as_deref(), Some("7"));
    assert_eq!(polled.started[0].label, "Dana");
    let sent = &work.received("poll_links")[0]["params"];
    assert_eq!(
        (sent["cursor"].as_str(), sent["wait_secs"].as_u64()),
        (Some("6"), Some(25))
    );
    notifier
        .poll_links(&config, None, Duration::from_millis(1500))
        .await
        .unwrap();
    let sent = &work.received("poll_links")[1]["params"];
    assert!(
        sent.get("cursor").is_none(),
        "the first reading has no cursor"
    );
    assert_eq!(sent["wait_secs"], 2, "whole seconds, rounded up");
    plugin.shutdown().await;
}

#[tokio::test]
async fn waiting_for_who_followed_a_link_may_take_as_long_as_it_was_told_to_wait_and_then_the_usual_time()
 {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &registration(
            PluginKind::Notifier,
            "chat",
            &work,
            &notifier_script(&json!({})),
            &["--hang-on", "poll_links"],
        ),
        &clock,
    )
    .await;
    let notifier = NotifierBridge::new(plugin.clone()).unwrap();
    let config = Config::default();
    let polling = tokio::spawn({
        let notifier = NotifierBridge::new(plugin.clone()).unwrap();
        async move {
            notifier
                .poll_links(&config, None, Duration::from_secs(25))
                .await
        }
    });
    wait_until("the plugin has the request", || {
        !work.received("poll_links").is_empty()
    })
    .await;
    advance(&clock, 84);
    settle().await;
    assert!(!polling.is_finished(), "25 s of waiting and 60 s to answer");
    advance(&clock, 1);
    let late = polling.await.unwrap().unwrap_err();
    assert!(
        late.retryable,
        "a plugin that did not answer may be asked again"
    );
    assert_eq!(late.code, "timeout");
    drop(notifier);
    plugin.shutdown().await;
}

struct VolumeWorkProbe {
    input_ready: std::sync::atomic::AtomicUsize,
    allowance: u64,
    command: orch_types::Id,
    reports: std::sync::Mutex<Vec<orch_driver_contract::volumes::VolumeProgress>>,
    cancelled: std::sync::atomic::AtomicBool,
    cancellation: tokio::sync::Notify,
}
impl VolumeWorkProbe {
    fn new() -> std::sync::Arc<Self> {
        Self::with_allowance(37)
    }
    fn with_allowance(allowance: u64) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            allowance,
            input_ready: Default::default(),
            command: orch_types::new_id(),
            reports: Default::default(),
            cancelled: Default::default(),
            cancellation: Default::default(),
        })
    }
    fn cancel(&self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.cancellation.notify_waiters();
    }
}
#[async_trait::async_trait]
impl orch_driver_contract::volumes::VolumeWork for VolumeWorkProbe {
    fn input_ready(&self) {
        self.input_ready
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    fn command_id(&self) -> Option<orch_types::Id> {
        Some(self.command)
    }
    fn report(&self, progress: orch_driver_contract::volumes::VolumeProgress) {
        self.reports.lock().unwrap().push(progress);
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(std::sync::atomic::Ordering::SeqCst)
    }
    async fn cancelled(&self) {
        let notified = self.cancellation.notified();
        if !self.is_cancelled() {
            notified.await;
        }
    }
    // A nondefault allowance proves the bridge carries the actual caller budget unchanged.
    fn timeout_secs(&self) -> u64 {
        self.allowance
    }
}

#[tokio::test]
async fn external_owned_volume_calls_keep_selectors_budgets_and_request_fenced_live_counts() {
    use orch_driver_contract::volumes::{OwnedVolumeSelector, VolumeWork};
    let (clock, work) = (clock(), Work::new());
    let progress = |bytes| json!({"event":"driver.volume_progress","params":{"progress_id":{"$params":"progress_id"},"progress":{"files_done":1,"bytes_done":bytes,"directories_done":null,"files_total":1,"bytes_total":bytes,"message":"Copying owned data"}}});
    let responses = json!({
        "check_volume_carry":{"result":{}},
        "measure_volume":{"events":[progress(17),{"event":"driver.volume_progress","params":{"progress_id":THREAD,"progress":{"files_done":999,"bytes_done":999}}}],
            "result":{"file_count":1,"total_bytes":17,"file_states":{"roots":1,"objects":0,"journals":0,"total_bytes":17,"complete":true}}},
        "export_volume":{"events":[progress(29)],"result":{}},
        "import_volume":{"events":[progress(31), {"event":"driver.volume_input_ready","params":{"progress_id":THREAD}}, {"event":"driver.volume_input_ready","params":{"progress_id":{"$params":"progress_id"}}}],"result":{}},
        "stop_volume_work":{"result":{}},
    });
    let methods = driver_methods(&[
        "check_volume_carry",
        "measure_volume",
        "export_volume",
        "import_volume",
        "stop_volume_work",
    ]);
    let plugin = start(
        &registration(
            PluginKind::Driver,
            "scripted",
            &work,
            &script(PluginKind::Driver, &methods, &responses),
            &[],
        ),
        &clock,
    )
    .await;
    let driver = DriverBridge::new(plugin.clone()).unwrap();
    let volume = VolumeRequest {
        name: "opaque-owned-volume".into(),
        storage: Some("durable".into()),
        labels: Default::default(),
    };
    let reports = VolumeWorkProbe::new();
    let reporting: std::sync::Arc<dyn VolumeWork> = reports.clone();
    driver.check_volume_carry().await.unwrap();
    let measured = driver
        .measure_volume(&volume, OwnedVolumeSelector::FileStates, &reporting)
        .await
        .unwrap();
    assert_eq!(measured.total_bytes, 17);
    assert_eq!(measured.file_states.unwrap().roots, 1);
    let path = std::path::Path::new("node-transfer.pipe").to_path_buf();
    driver
        .export_volume(&volume, OwnedVolumeSelector::FileStates, &path, &reporting)
        .await
        .unwrap();
    driver
        .import_volume(&volume, OwnedVolumeSelector::FileStates, &path, &reporting)
        .await
        .unwrap();
    driver.stop_volume_work(reports.command).await.unwrap();
    assert_eq!(
        reports
            .input_ready
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "only the active import token opens its input stream"
    );
    assert_eq!(
        reports
            .reports
            .lock()
            .unwrap()
            .iter()
            .map(|report| report.bytes_done)
            .collect::<Vec<_>>(),
        [17, 29, 31]
    );
    let mut tokens = std::collections::BTreeSet::new();
    for method in ["measure_volume", "export_volume", "import_volume"] {
        let received = work.received(method);
        let params = &received[0]["params"];
        assert_eq!(params["volume"], json!(volume));
        assert_eq!(params["selector"], "file_states");
        assert_eq!(params["timeout_secs"], 37);
        assert_eq!(params["command_id"], json!(reports.command));
        assert!(
            tokens.insert(params["progress_id"].as_str().unwrap().to_owned()),
            "each request gets a fresh progress identity"
        );
    }
    assert_eq!(
        work.received("export_volume")[0]["params"]["into"],
        json!(path)
    );
    assert_eq!(
        work.received("import_volume")[0]["params"]["from"],
        json!(path)
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn cancelling_external_owned_volume_work_stops_its_exact_command_and_missing_helpers_refuse_before_io()
 {
    use orch_driver_contract::volumes::{OwnedVolumeSelector, VolumeWork};
    let (clock, work) = (clock(), Work::new());
    let methods = driver_methods(&[
        "check_volume_carry",
        "measure_volume",
        "export_volume",
        "import_volume",
        "stop_volume_work",
    ]);
    let plugin = start(
        &registration(
            PluginKind::Driver,
            "scripted",
            &work,
            &script(
                PluginKind::Driver,
                &methods,
                &json!({"stop_volume_work":{"result":{}}}),
            ),
            &["--hang-on", "export_volume"],
        ),
        &clock,
    )
    .await;
    let driver = std::sync::Arc::new(DriverBridge::new(plugin.clone()).unwrap());
    let reports = VolumeWorkProbe::new();
    let reporting: std::sync::Arc<dyn VolumeWork> = reports.clone();
    let exporting = {
        let driver = driver.clone();
        tokio::spawn(async move {
            driver
                .export_volume(
                    &VolumeRequest {
                        name: "owned".into(),
                        storage: None,
                        labels: Default::default(),
                    },
                    OwnedVolumeSelector::Home,
                    std::path::Path::new("node-transfer.pipe"),
                    &reporting,
                )
                .await
        })
    };
    wait_until("the plugin receives the export", || {
        !work.received("export_volume").is_empty()
    })
    .await;
    reports.cancel();
    assert!(matches!(
        exporting.await.unwrap(),
        Err(DriverError::Timeout)
    ));
    assert_eq!(
        work.received("stop_volume_work")[0]["params"]["command_id"],
        json!(reports.command)
    );
    let timed = VolumeWorkProbe::with_allowance(1);
    let reporting: std::sync::Arc<dyn VolumeWork> = timed.clone();
    let pending = {
        let driver = driver.clone();
        tokio::spawn(async move {
            driver
                .export_volume(
                    &VolumeRequest {
                        name: "owned".into(),
                        storage: None,
                        labels: Default::default(),
                    },
                    OwnedVolumeSelector::Home,
                    std::path::Path::new("node-transfer.pipe"),
                    &reporting,
                )
                .await
        })
    };
    wait_until("the plugin receives the timed export", || {
        work.received("export_volume").len() == 2
    })
    .await;
    // Cross a one-second caller budget before the plugin's independent heartbeat deadline.
    advance(&clock, 2);
    assert!(matches!(
        tokio::time::timeout(support::PATIENCE, pending)
            .await
            .expect("the caller deadline must finish")
            .unwrap(),
        Err(DriverError::Timeout)
    ));
    assert_eq!(
        work.received("stop_volume_work")[1]["params"]["command_id"],
        json!(timed.command)
    );
    plugin.shutdown().await;
    let incomplete = Work::new();
    let plugin = start(
        &registration(
            PluginKind::Driver,
            "scripted",
            &incomplete,
            &script(
                PluginKind::Driver,
                &driver_methods(&["measure_volume"]),
                &json!({}),
            ),
            &[],
        ),
        &clock,
    )
    .await;
    let driver = DriverBridge::new(plugin.clone()).unwrap();
    assert!(
        matches!(driver.check_volume_carry().await,Err(DriverError::Unsupported(reason)) if reason.contains("stop_volume_work"))
    );
    assert!(incomplete.received("check_volume_carry").is_empty());
    plugin.shutdown().await;
}

// ---- driver -----------------------------------------------------------------------------------------------

fn unit() -> UnitRef {
    serde_json::from_value(json!({
        "reference": "c-1", "name": "orch-environment-0199abcd1234", "runtime_id": RUNTIME, "kind": "environment"
    }))
    .unwrap()
}

fn runtime_spec() -> RuntimeSpec {
    serde_json::from_value(json!({
        "runtime_id": RUNTIME, "kind": "environment", "name": "orch-environment-0199abcd1234", "driver": "scripted",
        "spec": { "cpus": 2 }, "resources": {}, "roles": ["environment"], "gateway_url": "wss://g", "enroll_token": "t",
        "ca_cert": "", "tools": [], "volumes": {}, "mounts": [], "env": {}, "labels": {}
    }))
    .unwrap()
}

const DRIVER_REQUIRED: [&str; 9] = [
    "describe",
    "create",
    "start",
    "stop",
    "remove",
    "inspect",
    "list",
    "ensure_volume",
    "remove_volume",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_external_driver_reports_storage_download_progress_suspend_and_typed_capacity_errors() {
    struct Lines(std::sync::Mutex<Vec<String>>);
    impl orch_driver_contract::sources::ImageProgress for Lines {
        fn line(&self, _: orch_driver_contract::sources::ImageStep, text: &str) {
            self.0.lock().unwrap().push(text.into());
        }
    }
    let (clock, work) = (clock(), Work::new());
    let responses = json!({
        "storages": {"result":{"storages":[{"name":"fast","default":true,"capacity_gb":10,"free_gb":2,"private_views":false}]}},
        "host_resources": {"result":{"cpus":12,"cpu_millis":11500,"memory_mb":24576,"disk_gb":900,"cpu_percent":125.0,"memory_used_mb":2048,"disk_used_gb":300,"disk_free_gb":600}},
        "configure": {"result":{}},
        "volume_exists": {"result":{"exists":true}},
        "suspend": {"result":{}}, "resume": {"result":{}},
        "ensure_image": {"progress":[{"message":"pulling verified layers","done":1,"total":2}], "result":{"digest":"sha256:abc"}},
        "create": {"error":{"code":"no_capacity","message":"the named storage is full","retryable":false}},
        "start": {"error":{"code":"busy","message":"the unit is held for transfer","retryable":true}},
    });
    let methods = driver_methods(&[
        "storages",
        "host_resources",
        "configure",
        "volume_exists",
        "suspend",
        "resume",
        "ensure_image",
    ]);
    let mut declared = script(PluginKind::Driver, &methods, &responses);
    declared["descriptor"]["suspend"] = json!(true);
    declared["descriptor"]["configuration_schema"] = json!({"type":"object","properties":{"namespace":{"type":"string","minLength":1}},"required":["namespace"],"additionalProperties":false});
    let plugin = start(
        &registration(PluginKind::Driver, "scripted", &work, &declared, &[]),
        &clock,
    )
    .await;
    let driver = DriverBridge::new(plugin.clone()).unwrap();
    let storage = driver.storages().unwrap();
    assert_eq!(driver.host_resources().unwrap().memory_mb, 24576);
    assert_eq!(driver.host_resources().unwrap().cpu_millis, Some(11500));
    driver
        .configure(&json!({"namespace":"agents"}))
        .await
        .unwrap();
    assert_eq!(
        work.received("configure")[0]["params"]["configuration"],
        json!({"namespace":"agents"})
    );
    assert!(
        driver
            .validate_configuration(&json!({"namespace":"agents"}))
            .is_ok()
    );
    for invalid in [
        json!({"namespace":17}),
        json!({"namespace":""}),
        json!({"namespace":"agents","other":true}),
    ] {
        assert!(matches!(
            driver.validate_configuration(&invalid),
            Err(DriverError::InvalidSpec(_))
        ));
    }
    assert_eq!((storage[0].name.as_str(), storage[0].free_gb), ("fast", 2));
    assert!(
        driver
            .volume_exists(&VolumeRequest {
                name: "owned".into(),
                storage: Some("fast".into()),
                labels: BTreeMap::new()
            })
            .await
            .unwrap()
    );
    driver.suspend(&unit()).await.unwrap();
    driver.resume(&unit()).await.unwrap();
    assert_eq!(work.received("suspend")[0]["params"]["unit"], json!(unit()));
    assert_eq!(work.received("resume")[0]["params"]["unit"], json!(unit()));
    let lines = Lines(std::sync::Mutex::new(Vec::new()));
    assert_eq!(
        driver
            .ensure_image_reporting(&json!({"image":"example:1"}), &lines)
            .await
            .unwrap()
            .digest
            .as_deref(),
        Some("sha256:abc")
    );
    assert_eq!(
        lines.0.lock().unwrap().as_slice(),
        ["pulling verified layers"]
    );
    assert!(
        matches!(driver.create(&runtime_spec()).await, Err(DriverError::NoCapacity(message)) if message == "the named storage is full")
    );
    assert!(
        matches!(driver.start(&unit()).await, Err(DriverError::Busy(message)) if message == "the unit is held for transfer")
    );
    plugin.shutdown().await;
}

fn driver_methods(optional: &[&'static str]) -> Vec<&'static str> {
    DRIVER_REQUIRED.iter().chain(optional).copied().collect()
}

#[tokio::test]
async fn every_method_of_the_driver_trait_reaches_the_plugin_with_its_parameters_and_brings_back_its_result()
 {
    let (clock, work) = (clock(), Work::new());
    let responses = json!({
        "validate": { "result": {} },
        "create": { "result": unit() },
        "start": { "result": {} }, "stop": { "result": {} }, "remove": { "result": {} },
        "inspect": { "result": { "state": "running", "detail": null, "exit_code": null, "started_at": null, "finished_at": null } },
        "list": { "result": { "units": [unit()] } },
        "stats": { "result": {
            "at": "2026-10-11T10:00:00.000Z", "cpu_percent": 12.5, "cpu_limit_millis": 2000, "memory_bytes": 1048576,
            "memory_limit_bytes": null, "disk_used_bytes": null, "disk_limit_bytes": null, "net_rx_bps": 0,
            "net_tx_bps": 0, "pids": 7 } },
        "ensure_volume": { "result": {} }, "remove_volume": { "result": {} },
        "ensure_image": { "progress": [ { "message": "pulling", "done": 1, "total": 2 } ], "result": { "digest": "sha256:abc" } },
        "import": { "result": { "spec": { "image": "debian" }, "notes": [ { "level": "info", "text": "read a FROM" } ] } }
    });
    let methods = driver_methods(&["validate", "stats", "ensure_image", "import"]);
    let plugin = start(
        &registration(
            PluginKind::Driver,
            "scripted",
            &work,
            &script(PluginKind::Driver, &methods, &responses),
            &[],
        ),
        &clock,
    )
    .await;
    let driver = DriverBridge::new(plugin.clone()).unwrap();
    assert_eq!(driver.describe().name, "scripted");
    assert_eq!(driver.describe().check(), Ok(()));
    assert_eq!(
        plugin.report().unavailable,
        [
            "rebuild_image",
            "registry_digest",
            "storages",
            "resources",
            "remove_resource",
            "stop_resource",
            "host_resources",
            "configure",
            "check_volume_carry",
            "measure_volume",
            "export_volume",
            "import_volume",
            "stop_volume_work",
            "volume_exists",
            "suspend",
            "resume",
            "apply_resources",
            "apply_capabilities",
            "estimate_keep",
            "keep_files",
            "kept_images",
            "remove_kept_image",
            "sweep_kept_metadata",
            "export_kept_image",
            "import_kept_image",
        ]
    );

    driver
        .validate(RuntimeKind::Environment, &json!({ "cpus": 2 }))
        .await
        .unwrap();
    assert_eq!(
        work.received("validate")[0]["params"],
        json!({ "kind": "environment", "spec": { "cpus": 2 } })
    );

    assert_eq!(driver.create(&runtime_spec()).await.unwrap(), unit());
    assert_eq!(
        work.received("create")[0]["params"]["name"],
        "orch-environment-0199abcd1234"
    );
    assert_eq!(
        work.received("create")[0]["params"]["spec"],
        json!({ "cpus": 2 })
    );

    driver.start(&unit()).await.unwrap();
    driver
        .stop(&unit(), Duration::from_millis(1500))
        .await
        .unwrap();
    driver.remove(&unit()).await.unwrap();
    assert_eq!(
        work.received("start")[0]["params"]["unit"]["reference"],
        "c-1"
    );
    assert_eq!(
        work.received("stop")[0]["params"]["grace_secs"],
        2,
        "whole seconds, rounded up"
    );
    assert_eq!(
        work.received("remove")[0]["params"]["unit"]["name"],
        "orch-environment-0199abcd1234"
    );

    assert_eq!(
        driver.inspect(&unit()).await.unwrap().state,
        UnitStateKind::Running
    );
    assert_eq!(driver.list("test-system").await.unwrap(), vec![unit()]);
    assert_eq!(
        work.received("list")[0]["params"],
        json!({ "system": "test-system" })
    );
    assert_eq!(driver.stats(&unit()).await.unwrap().pids, 7);

    let volume: VolumeRequest = serde_json::from_value(json!({
        "name": "orch-ws-1", "storage": null, "labels": { "orch.system": "test-system" }
    }))
    .unwrap();
    driver.ensure_volume(&volume).await.unwrap();
    driver.remove_volume("orch-ws-1").await.unwrap();
    assert_eq!(
        work.received("ensure_volume")[0]["params"]["labels"]["orch.system"],
        "test-system"
    );
    assert_eq!(
        work.received("remove_volume")[0]["params"],
        json!({ "name": "orch-ws-1" })
    );

    assert_eq!(
        driver
            .ensure_image(&json!({ "image": "debian" }))
            .await
            .unwrap()
            .digest
            .as_deref(),
        Some("sha256:abc")
    );
    let proposal = driver
        .import(&ImportRequest {
            kind: RuntimeKind::Environment,
            format: "dockerfile".into(),
            values: json!({}),
        })
        .await
        .unwrap();
    assert_eq!(proposal.spec, json!({ "image": "debian" }));
    plugin.shutdown().await;
}

#[tokio::test]
async fn an_optional_method_the_driver_did_not_list_is_not_asked_and_answers_as_the_traits_default_does()
 {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &registration(
            PluginKind::Driver,
            "scripted",
            &work,
            &script(PluginKind::Driver, &driver_methods(&[]), &json!({})),
            &[],
        ),
        &clock,
    )
    .await;
    let driver = DriverBridge::new(plugin.clone()).unwrap();
    assert_eq!(
        driver.validate(RuntimeKind::Environment, &json!({})).await,
        Ok(())
    );
    assert_eq!(
        driver.ensure_image(&json!({})).await.unwrap().digest,
        None,
        "a driver without images has nothing to make available"
    );
    assert_eq!(
        driver.stats(&unit()).await,
        Err(DriverError::Unsupported("stats".into()))
    );
    assert_eq!(
        driver
            .import(&ImportRequest {
                kind: RuntimeKind::Provider,
                format: "image".into(),
                values: json!({})
            })
            .await,
        Err(DriverError::Unsupported("import".into()))
    );
    for method in ["validate", "stats", "ensure_image", "import"] {
        assert!(work.received(method).is_empty(), "{method} was not asked");
    }
    plugin.shutdown().await;
}

#[tokio::test]
async fn the_errors_of_a_driver_keep_their_codes_and_a_plugin_that_is_down_is_unavailable() {
    let (clock, work) = (clock(), Work::new());
    let marker = work.text("once");
    let responses = json!({ "create": [
        { "error": { "code": "invalid_spec", "message": "no such cpu count", "retryable": false } },
        { "error": { "code": "image_failed", "message": "pull denied", "retryable": false } },
        { "error": { "code": "firecracker.no_kvm", "message": "no /dev/kvm", "retryable": false } }
    ], "inspect": { "result": {} } });
    let plugin = start(
        &registration(
            PluginKind::Driver,
            "scripted",
            &work,
            &script(PluginKind::Driver, &driver_methods(&[]), &responses),
            &["--exit-on", "list", "--once-marker", &marker],
        ),
        &clock,
    )
    .await;
    let driver = DriverBridge::new(plugin.clone()).unwrap();
    assert_eq!(
        driver.create(&runtime_spec()).await,
        Err(DriverError::InvalidSpec("no such cpu count".into()))
    );
    assert_eq!(
        driver.create(&runtime_spec()).await,
        Err(DriverError::Image("pull denied".into()))
    );
    assert_eq!(
        driver.create(&runtime_spec()).await,
        Err(DriverError::Internal(
            "firecracker.no_kvm: no /dev/kvm".into()
        )),
        "a code the contract does not know is kept"
    );
    // The answer of `inspect` is not a unit state.
    assert!(matches!(
        driver.inspect(&unit()).await,
        Err(DriverError::Internal(_))
    ));
    // It goes away in the middle of `list`, and is down afterwards: both are unavailable, which may pass.
    let lost = driver.list("test-system").await.unwrap_err();
    assert!(
        matches!(lost, DriverError::Unavailable(_)) && lost.retryable(),
        "{lost:?}"
    );
    wait_until("it waits to be restarted", || {
        plugin.report().retry_at.is_some()
    })
    .await;
    let down = driver.start(&unit()).await.unwrap_err();
    assert!(
        matches!(&down, DriverError::Unavailable(text) if text.contains("scripted")),
        "{down:?}"
    );
    plugin.shutdown().await;
}

// ---- executor ---------------------------------------------------------------------------------------------

fn environment() -> EnvironmentContext {
    serde_json::from_value(json!({
        "environment_id": "0199abcd-1234-7000-8000-0000000000e1", "generation": 3, "data_dir": "/var/lib/orch/exec/e1",
        "workspace_root": "/workspace", "home": null, "tools_dir": null, "tools": [], "mounts": [], "user": "agent",
        "shell": { "name": "sh", "path": "/bin/sh" }, "os": "linux", "arch": "x86_64", "path_style": "posix",
        "limits": {}, "variables": {}, "mcp_servers_enabled": false
    }))
    .unwrap()
}

#[tokio::test]
async fn an_executor_is_asked_whether_it_can_start_started_for_an_environment_and_stopped() {
    let (clock, work) = (clock(), Work::new());
    let responses = json!({
        "available": { "result": { "available": true, "version": "1.2.0", "reason": null, "sandbox_modes": ["open"] } },
        "start": { "result": { "address": "127.0.0.1:41231", "version": "1.2.0", "tool_sets": [], "sandbox_modes": ["open"] } },
        "stop": { "result": {} }
    });
    let plugin = start(
        &registration(
            PluginKind::Executor,
            "scripted",
            &work,
            &script(
                PluginKind::Executor,
                &["describe", "available", "start", "stop"],
                &responses,
            ),
            &[],
        ),
        &clock,
    )
    .await;
    let factory = ExecutorFactory::new(plugin.clone()).unwrap();
    let mut executor = factory.create().unwrap();
    assert_eq!(executor.kind(), "scripted-exec");
    assert_eq!(executor.describe().title, "A scripted executor");
    let available = executor.available(&environment()).await;
    assert!(available.available);
    assert_eq!(
        work.received("available")[0]["params"]["data_dir"],
        "/var/lib/orch/exec/e1"
    );
    let endpoint = executor.start(&environment()).await.unwrap();
    assert_eq!(endpoint.port(), Ok(41231));
    executor.stop().await.unwrap();
    assert_eq!(work.received("stop").len(), 1);
    // Two executors made by one factory are two executors of the one program.
    assert_eq!(factory.create().unwrap().kind(), "scripted-exec");
    plugin.shutdown().await;
}

#[tokio::test]
async fn an_address_that_is_not_on_the_machine_is_refused_and_what_was_started_is_stopped_again() {
    let (clock, work) = (clock(), Work::new());
    let responses = json!({
        "available": { "result": { "available": false, "version": null, "reason": "no build here", "sandbox_modes": [] } },
        "start": { "result": { "address": "10.0.0.5:7000", "version": "1.2.0" } },
        "stop": { "result": {} }
    });
    let plugin = start(
        &registration(
            PluginKind::Executor,
            "scripted",
            &work,
            &script(
                PluginKind::Executor,
                &["describe", "available", "start", "stop"],
                &responses,
            ),
            &[],
        ),
        &clock,
    )
    .await;
    let mut executor = ExecutorBridge::new(plugin.clone()).unwrap();
    let refused = executor.start(&environment()).await.unwrap_err();
    let refused = refused.to_string();
    assert!(
        refused.contains("loopback address only") && refused.contains("10.0.0.5"),
        "{refused}"
    );
    assert_eq!(work.received("stop").len(), 1, "it was stopped");
    let availability = executor.available(&environment()).await;
    assert!(!availability.available);
    assert_eq!(availability.reason.as_deref(), Some("no build here"));
    plugin.shutdown().await;
}

// ---- translator -------------------------------------------------------------------------------------------

fn upstream() -> UpstreamSide {
    UpstreamSide {
        base_url: "https://beta.example.com".into(),
        options: json!({ "region": "eu" }),
        credential: Some(Credential::new("sk-secret-key")),
    }
}

fn translator_script() -> Json {
    script(
        PluginKind::Translator,
        &[
            "describe",
            "translate_request",
            "response_translator",
            "translate_chunk",
            "finish_response",
        ],
        &json!({
            "translate_request": [
                { "result": {
                    "method": "POST", "path": "/beta/chat", "query": { "v": "2" },
                    "headers": { "x-beta-key": { "$params": "upstream.credential" } },
                    "body": { "$upper": { "$params": "body" } }, "authorized": true } },
                { "error": { "code": "malformed", "message": "not an alpha request", "retryable": false } },
                { "error": { "code": "unsupported", "message": "audio has no equivalent", "retryable": false } }
            ],
            "response_translator": [ { "result": { "converter": "c-1" } }, { "result": { "converter": "c-2" } } ],
            "translate_chunk": { "result": { "chunk": { "$upper": { "$params": "chunk" } } } },
            "finish_response": { "result": { "chunk": "[done]" } }
        }),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_translator_converts_a_request_and_the_response_stream_through_the_synchronous_trait() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &registration(
            PluginKind::Translator,
            "alpha-beta",
            &work,
            &translator_script(),
            &[],
        ),
        &clock,
    )
    .await;
    let translator = TranslatorBridge::new(plugin.clone()).unwrap();
    assert_eq!((translator.from(), translator.to()), ("alpha", "beta"));
    assert!(
        translator.describe().losses.has_losses(),
        "the binding dialog shows the losses"
    );

    let request = translator
        .translate_request("/chat", Bytes::from_static(b"ping ping"), &upstream())
        .unwrap();
    assert_eq!(
        (request.method.as_str(), request.path.as_str()),
        ("POST", "/beta/chat")
    );
    assert_eq!(request.body, "PING PING");
    assert!(
        request.authorized,
        "the translator presented the credential itself"
    );
    assert_eq!(
        request.headers["x-beta-key"], "sk-secret-key",
        "the credential travelled in that one request"
    );
    assert_eq!(request.query["v"], "2");
    let sent = &work.received("translate_request")[0]["params"];
    assert_eq!(
        (
            sent["path"].as_str(),
            sent["upstream"]["options"]["region"].as_str()
        ),
        (Some("/chat"), Some("eu"))
    );

    // A response arrives in pieces that may end inside a character: the program is given whole characters.
    let mut response = translator.response_translator();
    assert_eq!(
        response
            .translate_chunk(Bytes::from_static(b"caf\xc3"))
            .unwrap(),
        "CAF"
    );
    assert_eq!(
        response
            .translate_chunk(Bytes::from_static(b"\xa9 ok"))
            .unwrap(),
        "\u{c9} OK"
    );
    assert_eq!(
        response
            .translate_chunk(Bytes::from_static(b"\xe4"))
            .unwrap(),
        "",
        "only the beginning of a character"
    );
    assert_eq!(
        response.finish(),
        Err(TranslateError::Malformed(
            "the response ended in the middle of a character".into()
        ))
    );
    let chunks: Vec<_> = work
        .received("translate_chunk")
        .iter()
        .map(|line| {
            (
                line["params"]["converter"].as_str().unwrap().to_owned(),
                line["params"]["chunk"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        chunks,
        [
            ("c-1".into(), "caf".into()),
            ("c-1".into(), "\u{e9} ok".into())
        ]
    );
    assert_eq!(
        work.received("response_translator").len(),
        1,
        "one converter for one response"
    );

    let mut second = translator.response_translator();
    assert_eq!(second.finish().unwrap(), "[done]");
    assert_eq!(work.received("response_translator").len(), 2);
    assert_eq!(
        work.received("finish_response").last().unwrap()["params"]["converter"],
        "c-2"
    );
    drop(second);
    drop(response);
    plugin.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conversion_that_fails_says_why_and_a_response_that_is_given_up_is_finished_for_the_program()
 {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &registration(
            PluginKind::Translator,
            "alpha-beta",
            &work,
            &translator_script(),
            &[],
        ),
        &clock,
    )
    .await;
    let translator = TranslatorBridge::new(plugin.clone()).unwrap();
    let at = |plugin: &TranslatorBridge| {
        plugin.translate_request("/chat", Bytes::from_static(b"x"), &upstream())
    };
    translator
        .translate_request("/chat", Bytes::from_static(b"x"), &upstream())
        .unwrap();
    assert_eq!(
        at(&translator),
        Err(TranslateError::Malformed("not an alpha request".into()))
    );
    assert_eq!(
        at(&translator),
        Err(TranslateError::Unsupported(
            "audio has no equivalent".into()
        ))
    );
    assert!(matches!(
        translator.translate_request("/chat", Bytes::from_static(b"\xff\xfe"), &upstream()),
        Err(TranslateError::Malformed(_))
    ));

    let mut abandoned = translator.response_translator();
    abandoned.translate_chunk(Bytes::from_static(b"a")).unwrap();
    drop(abandoned);
    wait_until("the program was told the response is over", || {
        !work.received("finish_response").is_empty()
    })
    .await;
    let never_opened = translator.response_translator();
    drop(never_opened);
    settle().await;
    assert_eq!(
        work.received("finish_response").len(),
        1,
        "a response that never began opened nothing"
    );
    plugin.shutdown().await;
}

// ---- the set a component holds -----------------------------------------------------------------------------

#[tokio::test]
async fn a_component_gets_the_plugins_it_hosts_as_the_traits_of_their_contracts_and_lists_the_rest_as_they_are()
 {
    let (clock, work) = (clock(), Work::new());
    let rules = example_entry(
        PluginKind::Summarizer,
        "rules",
        "summarizer",
        "summarizer.py",
        &[],
    );
    let broken = registration(
        PluginKind::Summarizer,
        "old",
        &work,
        &script(
            PluginKind::Summarizer,
            &["describe", "summarize"],
            &json!({}),
        ),
        &["--hello-contract", "7"],
    );
    let elsewhere = example_entry(
        PluginKind::Notifier,
        "file",
        "notifier",
        "notifier.py",
        &["--file".into(), work.text("n.log")],
    );
    let plugins = Plugins::start(
        PluginHost::Api,
        &[rules, broken, elsewhere],
        &BuiltIn::new().with(PluginKind::Summarizer, ["model"]),
        &options(&clock),
    )
    .await
    .unwrap();
    plugins.settled().await;
    assert_eq!(plugins.host(), PluginHost::Api);
    assert_eq!(plugins.all().len(), 2, "the notifier is the worker's");

    let summarizers = plugins.summarizers();
    assert_eq!(
        summarizers.len(),
        1,
        "a plugin that was refused is not an implementation"
    );
    assert_eq!(summarizers[0].0, "rules");
    assert_eq!(summarizers[0].1.describe().name, "rules");
    assert!(plugins.drivers().is_empty() && plugins.notifiers().is_empty());

    let reports = plugins.reports();
    assert_eq!(
        reports
            .iter()
            .map(|report| (report.name.as_str(), report.state))
            .collect::<Vec<_>>(),
        [
            ("rules", PluginState::Running),
            ("old", PluginState::Failed)
        ]
    );
    assert!(reports[1].detail.as_deref().unwrap().contains("[1, 1]"));
    let statuses = plugins.statuses();
    assert_eq!(
        (statuses[0].name.as_str(), statuses[0].version.as_str()),
        ("rules", "0.1.0")
    );
    assert!(
        plugins.get(PluginKind::Summarizer, "rules").is_some()
            && plugins.get(PluginKind::Driver, "rules").is_none()
    );
    plugins.shutdown().await;
}

#[tokio::test]
async fn a_bridge_is_only_made_of_a_plugin_that_is_of_its_kind_and_has_said_hello() {
    let (clock, work) = (clock(), Work::new());
    let plugin = start(
        &example_entry(
            PluginKind::Summarizer,
            "rules",
            "summarizer",
            "summarizer.py",
            &[],
        ),
        &clock,
    )
    .await;
    assert!(matches!(
        NotifierBridge::new(plugin.clone()),
        Err(BridgeError::WrongKind {
            expected: PluginKind::Notifier,
            found: PluginKind::Summarizer,
            ..
        })
    ));
    let silent = Plugin::start(
        &registration(
            PluginKind::Summarizer,
            "mute",
            &work,
            &script(
                PluginKind::Summarizer,
                &["describe", "summarize"],
                &json!({}),
            ),
            &["--silent-hello"],
        ),
        options(&clock),
    )
    .await
    .unwrap();
    wait_until("the program started", || silent.report().starts == 1).await;
    assert!(matches!(
        SummarizerBridge::new(silent.clone()),
        Err(BridgeError::NotNegotiated(_))
    ));
    silent.shutdown().await;
    plugin.shutdown().await;

    let registrations = [example_entry(
        PluginKind::Driver,
        "docker",
        "none",
        "none",
        &[],
    )];
    let taken = hosted_by(
        PluginHost::Node,
        &registrations,
        &BuiltIn::new().with(PluginKind::Driver, ["docker"]),
    );
    assert!(
        matches!(taken, Err(LoadError::Entry(_))),
        "a built-in implementation's name is taken"
    );
}

#[tokio::test]
async fn driver_inventory_keeps_generic_kinds_and_opaque_ids_through_the_external_boundary() {
    use orch_driver_contract::resources::{ResourceKind, ResourceRef};
    let (clock, work) = (clock(), Work::new());
    let reference = ResourceRef {
        kind: ResourceKind::Volume,
        id: "pool-object:opaque-version".into(),
    };
    let unit_reference = ResourceRef {
        kind: ResourceKind::Unit,
        id: "pool-unit:opaque-incarnation".into(),
    };
    let responses = json!({"resources":{"result":{"resources":[{"resource":reference,"name":"durable data","size_bytes":null,"labels":{"orch.system":"test-system"},"depends_on":[],"last_used_at":null,"busy":false}]}},"remove_resource":{"result":{}},"stop_resource":{"result":{}}});
    let plugin = start(
        &registration(
            PluginKind::Driver,
            "scripted",
            &work,
            &script(
                PluginKind::Driver,
                &driver_methods(&["resources", "remove_resource", "stop_resource"]),
                &responses,
            ),
            &[],
        ),
        &clock,
    )
    .await;
    let driver = DriverBridge::new(plugin.clone()).unwrap();
    let listed = driver.resources("test-system").await.unwrap();
    assert_eq!(listed[0].resource, reference);
    assert!(listed[0].size_bytes.is_none());
    driver
        .remove_resource("test-system", &reference)
        .await
        .unwrap();
    assert_eq!(
        work.received("resources")[0]["params"],
        json!({"system":"test-system"})
    );
    assert_eq!(
        work.received("remove_resource")[0]["params"],
        json!({"system":"test-system","resource":reference})
    );
    assert!(matches!(
        driver
            .stop_resource("test-system", &reference, Duration::ZERO)
            .await,
        Err(DriverError::InvalidSpec(_))
    ));
    assert!(work.received("stop_resource").is_empty());
    driver
        .stop_resource("test-system", &unit_reference, Duration::from_millis(1500))
        .await
        .unwrap();
    assert_eq!(
        work.received("stop_resource")[0]["params"],
        json!({"system":"test-system","resource":unit_reference,"grace_secs":2})
    );
    plugin.shutdown().await;
    let missing = Work::new();
    let plugin = start(
        &registration(
            PluginKind::Driver,
            "scripted",
            &missing,
            &script(PluginKind::Driver, &driver_methods(&[]), &json!({})),
            &[],
        ),
        &clock,
    )
    .await;
    let driver = DriverBridge::new(plugin.clone()).unwrap();
    assert!(matches!(
        driver.resources("test-system").await,
        Err(DriverError::Unsupported(_))
    ));
    assert!(missing.received("resources").is_empty());
    assert!(matches!(
        driver
            .stop_resource("test-system", &unit_reference, Duration::ZERO)
            .await,
        Err(DriverError::Unsupported(_))
    ));
    assert!(missing.received("stop_resource").is_empty());
    plugin.shutdown().await;
}
