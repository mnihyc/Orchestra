//! Native settings are future thread defaults; their observations never replace the active turn's choices.
use super::*;
use orch_api_contract::threads::MarkerHarnessChanged;

fn sandbox(value: &Json) -> Option<Json> {
    if let Some(mode) = value.as_str() {
        let kind = match mode {
            "read-only" => "readOnly",
            "workspace-write" => "workspaceWrite",
            "danger-full-access" => "dangerFullAccess",
            other => other,
        };
        return Some(json!({"type":kind}));
    }
    let source = value.as_object()?;
    let mut kept = serde_json::Map::new();
    for key in [
        "type",
        "writableRoots",
        "networkAccess",
        "excludeTmpdirEnvVar",
        "excludeSlashTmp",
    ] {
        if let Some(value) = source.get(key) {
            kept.insert(key.into(), value.clone());
        }
    }
    Some(Json::Object(kept))
}

/// Only reported/requested fields participate. Instructions and opaque config objects have no display port.
pub(super) fn snapshot(value: &Json) -> BTreeMap<String, Json> {
    let mut values = BTreeMap::new();
    if let Some(model) = value
        .get("model")
        .and_then(Json::as_str)
        .filter(|model| !model.trim().is_empty())
    {
        values.insert("model".into(), json!(model));
    }
    for wire in ["effort", "reasoningEffort"] {
        if let Some(value) = value.get(wire).filter(|value| {
            value.is_null() || value.as_str().is_some_and(|effort| !effort.is_empty())
        }) {
            values.insert("effort".into(), value.clone());
        }
    }
    if let Some(policy) = value.get("approvalPolicy") {
        let mut approval = serde_json::Map::new();
        if policy.is_string() {
            approval.insert("policy".into(), policy.clone());
        } else if let Some(reject) = policy.get("reject").and_then(Json::as_object) {
            approval.insert(
                "reject".into(),
                Json::Object(
                    reject
                        .iter()
                        .filter(|(_, value)| value.is_boolean())
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                ),
            );
        }
        if let Some(reviewer) = value
            .get("approvalsReviewer")
            .filter(|value| value.is_string())
        {
            approval.insert("reviewer".into(), reviewer.clone());
        }
        if !approval.is_empty() {
            values.insert("approval_mode".into(), Json::Object(approval));
        }
    }
    if let Some(value) = value
        .get("sandboxPolicy")
        .or_else(|| value.get("sandbox"))
        .and_then(sandbox)
    {
        values.insert("sandbox_mode".into(), value);
    }
    if let Some(mode) = value.get("collaborationMode").and_then(Json::as_object) {
        let mut kept = serde_json::Map::new();
        if let Some(value) = mode.get("mode") {
            kept.insert("mode".into(), value.clone());
        }
        if let Some(settings) = mode.get("settings") {
            let kept_settings: serde_json::Map<String, Json> = ["model", "reasoning_effort"]
                .into_iter()
                .filter_map(|key| settings.get(key).map(|value| (key.into(), value.clone())))
                .collect();
            kept.insert("settings".into(), Json::Object(kept_settings));
        }
        values.insert("collaboration_mode".into(), Json::Object(kept));
    }
    values
}

// A private instruction body participates only in comparison; neither body nor fingerprint is displayed.
fn instructions(value: &Json) -> Option<Json> {
    let value = value.pointer("/collaborationMode/settings/developer_instructions")?;
    match value {
        Json::Null => Some(Json::Null),
        Json::String(body) => Some(json!(Hash::of(body.as_bytes()).to_string())),
        _ => None,
    }
}
fn hidden(value: Json) -> Json {
    if value.is_null() {
        Json::Null
    } else {
        json!({"redacted":true})
    }
}

// Omitted report members are unknown, rather than evidence that an explicit requested member changed.
fn agrees(expected: &Json, reported: &Json) -> bool {
    match (expected.as_object(), reported.as_object()) {
        (Some(expected), Some(reported)) => reported.iter().all(|(key, value)| {
            expected
                .get(key)
                .is_none_or(|expected| agrees(expected, value))
        }),
        _ => expected == reported,
    }
}

#[derive(Debug, Default)]
pub(super) struct NativeSettings {
    expected: BTreeMap<String, Json>,
    observed: HashMap<String, BTreeMap<String, Json>>,
    compared: HashMap<String, BTreeMap<String, Option<Json>>>,
    states: HashMap<(String, String, Option<String>), Json>,
    serial: u64,
    expected_instructions: Option<Json>,
    observed_instructions: HashMap<String, Json>,
    compared_instructions: HashMap<String, Option<Json>>,
}
impl NativeSettings {
    pub(super) fn expect(&mut self, values: BTreeMap<String, Json>) {
        self.expected.extend(values);
    }
    pub(super) fn expect_instructions(&mut self, asked: &Json) {
        if let Some(value) = instructions(asked) {
            self.expected_instructions = Some(value);
        }
    }
    pub(super) fn opened(&mut self, thread: &str, value: &Json) {
        let value = value.get("threadSettings").unwrap_or(value);
        self.observed.insert(thread.to_owned(), snapshot(value));
        if let Some(value) = instructions(value) {
            self.observed_instructions.insert(thread.to_owned(), value);
        }
    }
    pub(super) fn state(
        &mut self,
        thread: &str,
        change: &str,
        subject: Option<String>,
        value: Json,
        turn: Option<Id>,
        at: Time,
    ) -> Option<(u64, MarkerHarnessChanged)> {
        let previous = self.states.insert(
            (thread.into(), change.into(), subject.clone()),
            value.clone(),
        );
        if previous.as_ref() == Some(&value) {
            return None;
        }
        self.serial = self.serial.saturating_add(1);
        let changed = previous.as_ref().is_some_and(|old| old != &value);
        Some((
            self.serial,
            MarkerHarnessChanged {
                program: "Codex".into(),
                change: change.into(),
                subject,
                previous: previous.map_or(
                    orch_api_contract::usage::UsageMeasurement::Unsupported,
                    |value| orch_api_contract::usage::UsageMeasurement::Told { value },
                ),
                value,
                expected: orch_api_contract::usage::UsageMeasurement::Unsupported,
                changed,
                diverged: false,
                next_turn: false,
                turn_id: turn,
                reported_at: at,
            },
        ))
    }

    pub(super) fn observe(
        &mut self,
        params: &Json,
        main: bool,
        turn: Option<Id>,
        subject: Option<String>,
        at: Time,
    ) -> Vec<(u64, MarkerHarnessChanged)> {
        let Some(settings) = params
            .get("threadSettings")
            .filter(|value| value.is_object())
        else {
            return Vec::new();
        };
        let thread = text(params, "threadId").to_owned();
        let observed = self.observed.entry(thread.clone()).or_default();
        let compared = self.compared.entry(thread).or_default();
        let mut result = Vec::new();
        for (change, value) in snapshot(settings) {
            let expected = main.then(|| self.expected.get(&change).cloned()).flatten();
            let previous = observed.insert(change.clone(), value.clone());
            let previous_expected = compared.insert(change.clone(), expected.clone());
            let diverged = expected
                .as_ref()
                .is_some_and(|expected| !agrees(expected, &value));
            if previous.as_ref() == Some(&value)
                && (!diverged || previous_expected.as_ref() == Some(&expected))
            {
                continue;
            }
            if previous.is_none()
                && expected
                    .as_ref()
                    .is_some_and(|expected| agrees(expected, &value))
            {
                continue;
            }
            self.serial = self.serial.saturating_add(1);
            let changed = previous.as_ref().is_some_and(|old| old != &value);
            result.push((
                self.serial,
                MarkerHarnessChanged {
                    program: "Codex".into(),
                    change,
                    subject: subject.clone(),
                    previous: previous.map_or(
                        orch_api_contract::usage::UsageMeasurement::Unsupported,
                        |value| orch_api_contract::usage::UsageMeasurement::Told { value },
                    ),
                    value,
                    expected: expected.map_or(
                        orch_api_contract::usage::UsageMeasurement::Unsupported,
                        |value| orch_api_contract::usage::UsageMeasurement::Told { value },
                    ),
                    changed,
                    diverged,
                    next_turn: true,
                    turn_id: turn,
                    reported_at: at,
                },
            ));
        }
        if let Some(value) = instructions(settings) {
            let thread = text(params, "threadId").to_owned();
            let previous = self
                .observed_instructions
                .insert(thread.clone(), value.clone());
            let expected = main.then(|| self.expected_instructions.clone()).flatten();
            let old_expected = self.compared_instructions.insert(thread, expected.clone());
            let diverged = expected.as_ref().is_some_and(|expected| expected != &value);
            let changed = previous.as_ref().is_some_and(|old| old != &value);
            if previous.as_ref() != Some(&value)
                || diverged && old_expected.as_ref() != Some(&expected)
            {
                self.serial = self.serial.saturating_add(1);
                result.push((
                    self.serial,
                    MarkerHarnessChanged {
                        program: "Codex".into(),
                        change: "collaboration_instructions".into(),
                        subject,
                        previous: previous.map(hidden).map_or(
                            orch_api_contract::usage::UsageMeasurement::Unsupported,
                            |value| orch_api_contract::usage::UsageMeasurement::Told { value },
                        ),
                        value: hidden(value),
                        expected: expected.map(hidden).map_or(
                            orch_api_contract::usage::UsageMeasurement::Unsupported,
                            |value| orch_api_contract::usage::UsageMeasurement::Told { value },
                        ),
                        changed,
                        diverged,
                        next_turn: true,
                        turn_id: turn,
                        reported_at: at,
                    },
                ));
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_new_request_expectation_can_reveal_an_unchanged_mismatch_once() {
        let mut settings = NativeSettings::default();
        settings.expect(snapshot(&json!({"model":"first"})));
        settings.opened("thread", &json!({"model":"first"}));
        let at = Time::new(chrono::Utc::now());
        let actual = json!({"threadId":"thread","threadSettings":{"model":"first"}});
        assert!(settings.observe(&actual, true, None, None, at).is_empty());
        settings.expect(snapshot(&json!({"model":"second"})));
        let out = settings.observe(&actual, true, None, None, at);
        assert_eq!(out.len(), 1);
        assert!(out[0].1.diverged);
        assert_eq!(out[0].1.previous.value(), Some(&json!("first")));
        assert!(settings.observe(&actual, true, None, None, at).is_empty());
    }
    #[test]
    fn absent_model_and_partial_reports_never_invent_a_different_requested_value() {
        assert!(!snapshot(&json!({"model":"   "})).contains_key("model"));
        assert!(agrees(
            &json!({"type":"workspaceWrite","networkAccess":false}),
            &json!({"type":"workspaceWrite"})
        ));
        assert!(!agrees(
            &json!({"type":"workspaceWrite","networkAccess":false}),
            &json!({"type":"workspaceWrite","networkAccess":true})
        ));
    }
    #[test]
    fn private_instruction_changes_report_only_the_change_not_a_body_or_fingerprint() {
        let first = json!({"collaborationMode":{"mode":"default","settings":{"model":"model","developer_instructions":"private-first"}}});
        let mut settings = NativeSettings::default();
        settings.expect(snapshot(&first));
        settings.expect_instructions(&first);
        settings.opened("thread", &first);
        let next = json!({"threadId":"thread","threadSettings":{"collaborationMode":{"mode":"default","settings":{"model":"model","developer_instructions":"private-second"}}}});
        let reports = settings.observe(&next, true, None, None, Time::new(chrono::Utc::now()));
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].1.change, "collaboration_instructions");
        assert!(reports[0].1.changed);
        assert!(reports[0].1.diverged);
        let text = serde_json::to_string(&reports[0].1).unwrap();
        assert!(!text.contains("private-first"));
        assert!(!text.contains("private-second"));
        assert!(!text.contains(&Hash::of(b"private-first").to_string()));
        assert!(!text.contains(&Hash::of(b"private-second").to_string()));
    }
}
