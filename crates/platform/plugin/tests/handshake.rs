//! The handshake and the negotiation of versions and methods, with real programs.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use std::collections::BTreeMap;

use orch_plugin::supervisor::{Plugin, Versions};
use orch_plugin_contract::{PluginKind, PluginRegistration, PluginState};
use serde_json::json;
use support::{
    PYTHON, Work, advance, alive, clock, example, options, registration, script, settle, wait_until,
};

fn summarizer_script() -> serde_json::Value {
    script(
        PluginKind::Summarizer,
        &["describe", "summarize"],
        &json!({ "summarize": { "result": { "text": "ok", "tags": null, "usage": null } } }),
    )
}

#[tokio::test]
async fn a_plugin_that_speaks_the_offered_versions_is_running_and_listed_with_its_own_version() {
    let clock = clock();
    let rules = PluginRegistration {
        kind: PluginKind::Summarizer,
        name: "rules".into(),
        command: Some(vec![PYTHON.into(), example("summarizer", "summarizer.py")]),
        connect: None,
        env: BTreeMap::new(),
        restart: None,
    };
    let plugin = Plugin::start(&rules, options(&clock)).await.unwrap();
    plugin.settled().await;
    let report = plugin.report();
    assert_eq!(report.state, PluginState::Running);
    assert_eq!(
        report.version, "0.1.0",
        "the plugin's own version, as it writes it"
    );
    assert_eq!(
        report.versions,
        Some(Versions {
            transport: 1,
            contract: 1
        })
    );
    assert_eq!(
        (
            report.starts,
            report.detail.clone(),
            report.unavailable.len()
        ),
        (1, None, 0)
    );
    assert_eq!(
        report.status().state,
        PluginState::Running,
        "the part of it that machine facts carry"
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_version_below_or_above_what_the_host_offers_stops_the_plugin_and_says_what_each_side_speaks()
 {
    for (flag, value) in [
        ("--hello-transport", "0"),
        ("--hello-transport", "2"),
        ("--hello-contract", "0"),
        ("--hello-contract", "2"),
    ] {
        let clock = clock();
        let work = Work::new();
        let registration = registration(
            PluginKind::Summarizer,
            "rules",
            &work,
            &summarizer_script(),
            &[flag, value],
        );
        let plugin = Plugin::start(&registration, options(&clock)).await.unwrap();
        plugin.settled().await;
        let report = plugin.report();
        assert_eq!(report.state, PluginState::Failed, "{flag} {value}");
        let detail = report.detail.unwrap();
        assert!(
            detail.contains("[1, 1]"),
            "the host's versions are in: {detail}"
        );
        assert!(
            detail.contains(&format!("speaks {value}")),
            "the plugin's version is in: {detail}"
        );
        // A plugin that was refused would answer the same again: it is not started again.
        advance(&clock, 3600);
        settle().await;
        assert_eq!(work.pids().len(), 1, "{flag} {value}: started once");
        assert_eq!(plugin.report().starts, 1);
        assert!(!alive(work.pids()[0]), "the refused program was ended");
        plugin.shutdown().await;
    }
}

#[tokio::test]
async fn a_plugin_of_another_kind_than_the_one_registered_is_stopped() {
    let clock = clock();
    let work = Work::new();
    let registration = registration(
        PluginKind::Summarizer,
        "rules",
        &work,
        &summarizer_script(),
        &["--hello-kind", "notifier"],
    );
    let plugin = Plugin::start(&registration, options(&clock)).await.unwrap();
    plugin.settled().await;
    let report = plugin.report();
    assert_eq!(report.state, PluginState::Failed);
    assert!(
        report
            .detail
            .unwrap()
            .contains("expected a summarizer and the plugin is a notifier")
    );
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_required_method_that_is_missing_refuses_the_plugin_and_an_optional_one_only_shows_as_unavailable()
 {
    let clock = clock();
    let work = Work::new();
    let without_summarize = script(PluginKind::Summarizer, &["describe"], &json!({}));
    let refused = Plugin::start(
        &registration(
            PluginKind::Summarizer,
            "rules",
            &work,
            &without_summarize,
            &[],
        ),
        options(&clock),
    )
    .await
    .unwrap();
    refused.settled().await;
    let report = refused.report();
    assert_eq!(report.state, PluginState::Failed);
    assert!(
        report
            .detail
            .unwrap()
            .contains("lacks required methods: summarize")
    );
    refused.shutdown().await;

    let work = Work::new();
    let required_only = script(
        PluginKind::Driver,
        &[
            "describe",
            "create",
            "start",
            "stop",
            "remove",
            "inspect",
            "list",
            "ensure_volume",
            "remove_volume",
        ],
        &json!({}),
    );
    let accepted = Plugin::start(
        &registration(PluginKind::Driver, "scripted", &work, &required_only, &[]),
        options(&clock),
    )
    .await
    .unwrap();
    accepted.settled().await;
    let report = accepted.report();
    assert_eq!(report.state, PluginState::Running);
    assert_eq!(
        report.unavailable,
        [
            "validate",
            "stats",
            "ensure_image",
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
            "import"
        ],
        "what depends on a method the plugin did not list is shown as unavailable"
    );
    assert!(accepted.supports("create") && !accepted.supports("stats"));
    accepted.shutdown().await;
}

#[tokio::test]
async fn members_and_methods_that_are_not_known_are_tolerated_and_an_unknown_method_is_answered_unsupported()
 {
    let clock = clock();
    let work = Work::new();
    let registration = registration(
        PluginKind::Summarizer,
        "rules",
        &work,
        &summarizer_script(),
        &["--extras"],
    );
    let plugin = Plugin::start(&registration, options(&clock)).await.unwrap();
    plugin.settled().await;
    assert_eq!(plugin.report().state, PluginState::Running);
    let negotiated = plugin.negotiated().unwrap();
    assert!(
        negotiated
            .methods
            .declared()
            .any(|name| name == "x_snapshot"),
        "a method this host does not know is tolerated"
    );
    assert_eq!(
        negotiated.descriptor["x_note"], "kept",
        "members of its own are kept"
    );

    // Answers with members the host does not know are read all the same.
    let answered = plugin
        .call("summarize", json!({ "field": "title" }))
        .await
        .unwrap();
    assert_eq!(answered["text"], "ok");

    // A method the plugin does not know is answered with `unsupported`, and the plugin goes on.
    let refused = plugin.call("frobnicate", json!({})).await.unwrap_err();
    assert_eq!(refused.code(), "unsupported");
    assert!(!refused.retryable());
    assert_eq!(plugin.report().state, PluginState::Running);
    assert!(plugin.call("summarize", json!({})).await.is_ok());
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_descriptor_that_cannot_hold_together_stops_the_plugin_with_the_reason() {
    let clock = clock();
    let work = Work::new();
    let mut broken = summarizer_script();
    broken["descriptor"]["fields"] = json!([]);
    let plugin = Plugin::start(
        &registration(PluginKind::Summarizer, "rules", &work, &broken, &[]),
        options(&clock),
    )
    .await
    .unwrap();
    plugin.settled().await;
    let report = plugin.report();
    assert_eq!(report.state, PluginState::Failed);
    assert!(report.detail.unwrap().contains("at least one field"));
    plugin.shutdown().await;
}

#[tokio::test]
async fn a_hello_that_is_not_answered_in_time_is_a_failure_of_the_program_and_it_is_started_again()
{
    let clock = clock();
    let work = Work::new();
    let registration = registration(
        PluginKind::Summarizer,
        "rules",
        &work,
        &summarizer_script(),
        &["--silent-hello"],
    );
    let plugin = Plugin::start(&registration, options(&clock)).await.unwrap();
    wait_until("the program is running", || !work.pids().is_empty()).await;
    advance(&clock, 59);
    settle().await;
    assert_eq!(
        plugin.report().state,
        PluginState::Restarting,
        "still waiting"
    );
    assert!(plugin.report().detail.is_none());
    advance(&clock, 1);
    wait_until("the wait for hello was given up", || {
        plugin.report().detail.is_some()
    })
    .await;
    let report = plugin.report();
    assert_eq!(
        report.state,
        PluginState::Restarting,
        "a program that says nothing has failed like any"
    );
    assert!(
        report
            .detail
            .unwrap()
            .contains("did not answer hello within 60 s")
    );
    wait_until("the silent program was ended", || !alive(work.pids()[0])).await;
    advance(&clock, 1);
    wait_until("it was started again", || plugin.report().starts == 2).await;
    plugin.shutdown().await;
}
