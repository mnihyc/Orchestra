//! The metric `orch_plugins`: how many plugins there are, by kind and state.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_plugin::supervisor::Plugin;
use orch_plugin_contract::PluginKind;
use serde_json::json;
use support::{Work, advance, clock, options, registration, script, wait_until};

/// The value of the gauge for a kind and a state in the text a collector reads; none if it was never set.
fn plugins(text: &str, kind: &str, state: &str) -> Option<f64> {
    text.lines()
        .filter(|line| line.starts_with("orch_plugins{"))
        .find(|line| {
            line.contains(&format!("kind=\"{kind}\""))
                && line.contains(&format!("state=\"{state}\""))
        })
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse().ok())
}

#[tokio::test]
async fn the_number_of_plugins_by_kind_and_state_follows_them_through_a_restart_and_a_shutdown() {
    let readings = orch_telemetry::install_metrics().unwrap();
    let (clock, work) = (clock(), Work::new());
    let summarizer = script(
        PluginKind::Summarizer,
        &["describe", "summarize"],
        &json!({}),
    );
    let marker = work.text("once");
    let plugin = Plugin::start(
        &registration(
            PluginKind::Summarizer,
            "rules",
            &work,
            &summarizer,
            &["--exit-on", "die", "--once-marker", &marker],
        ),
        options(&clock),
    )
    .await
    .unwrap();
    plugin.settled().await;
    assert_eq!(
        plugins(&readings.render(), "summarizer", "running"),
        Some(1.0)
    );

    assert!(plugin.call("die", json!({})).await.is_err());
    wait_until("it waits to be restarted", || {
        plugin.report().retry_at.is_some()
    })
    .await;
    let text = readings.render();
    assert_eq!(plugins(&text, "summarizer", "running"), Some(0.0));
    assert_eq!(plugins(&text, "summarizer", "restarting"), Some(1.0));

    advance(&clock, 1);
    wait_until("it runs again", || {
        plugin.report().starts == 2 && plugin.report().retry_at.is_none()
    })
    .await;
    wait_until("the gauge shows it", || {
        plugins(&readings.render(), "summarizer", "running") == Some(1.0)
    })
    .await;

    plugin.shutdown().await;
    let text = readings.render();
    for state in ["running", "restarting", "failed"] {
        assert_eq!(
            plugins(&text, "summarizer", state).unwrap_or(0.0),
            0.0,
            "a plugin that is stopped is no longer counted as {state}"
        );
    }
}
