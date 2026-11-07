//! Observed 2.1.312 classifier protocol facts for the existing native conformance runner.
//! No successful classifier request is synthesized into a production approval-review item.
use orch_agent_contract::conformance::{
    NativeClassifierCase, NativeClassifierEvidence, NativeClassifierOutcome as Outcome,
    NativeClassifierReview, NativeClassifierStage, NativeRequestPattern,
};
use orch_agent_contract::descriptor::SandboxMode;
use orch_agent_contract::events::SessionEvent;
use orch_agent_contract::review::REVIEW_TOOL;
use orch_types::Json;
use serde_json::json;
use std::collections::BTreeMap;

const SONNET: &str = "claude-sonnet-5";
const OPUS: &str = "claude-opus-5";
const MONITOR: &str = "You are a security monitor for autonomous AI coding agents.";
const STAGE_ONE: &str = "Stage 1 does NOT apply user intent or ALLOW exceptions";
const SEVERITY_TWO: &str = "Use <thinking> first, then respond with <severity>N</severity>";

fn main_pattern(continuation: bool) -> NativeRequestPattern {
    let mut pattern = NativeRequestPattern::default();
    pattern.equals.insert("/stream".into(), json!(true));
    pattern.present = vec!["/tools".into(), "/messages".into()];
    if continuation {
        pattern
            .contains
            .insert("/messages".into(), vec!["tool_result".into()]);
    } else {
        pattern
            .excludes
            .insert("/messages".into(), vec!["tool_result".into()]);
    }
    pattern
}
fn signature(protocol: &str, stage: u8) -> NativeRequestPattern {
    assert!(
        matches!((protocol, stage), ("severity", 1 | 2) | ("block", 1)),
        "unknown classifier protocol/stage is not a recipe"
    );
    let mut pattern = NativeRequestPattern::default();
    pattern.absent = vec!["/tools".into(), "/stream".into()];
    pattern
        .equals
        .insert("/thinking/type".into(), json!("disabled"));
    pattern
        .contains
        .insert("/system".into(), vec![MONITOR.into()]);
    pattern.contains.insert(
        "/messages".into(),
        vec!["<transcript>".into(), "</transcript>".into()],
    );
    if stage == 1 {
        pattern.equals.insert("/max_tokens".into(), json!(64));
        pattern.equals.insert(
            "/stop_sequences".into(),
            json!([if protocol == "severity" {
                "</severity>"
            } else {
                "</block>"
            }]),
        );
        pattern
            .contains
            .get_mut("/messages")
            .expect("inserted above")
            .push(STAGE_ONE.into());
        if protocol == "severity" {
            pattern
                .contains
                .get_mut("/system")
                .expect("inserted above")
                .push("Output <severity>N</severity>".into());
            pattern
                .contains
                .get_mut("/messages")
                .expect("inserted above")
                .push("Respond with <severity>N</severity> ONLY".into());
        } else {
            pattern
                .contains
                .get_mut("/system")
                .expect("inserted above")
                .push("<block>no</block>".into());
            pattern
                .contains
                .get_mut("/messages")
                .expect("inserted above")
                .push("Your ENTIRE response MUST begin with <block>".into());
        }
    } else {
        // Only severity Stage2 has actual successful native evidence. Basic Stage2 is not guessed.
        pattern.equals.insert("/max_tokens".into(), json!(8192));
        pattern.absent.push("/stop_sequences".into());
        pattern
            .contains
            .get_mut("/system")
            .expect("inserted above")
            .push("Output <severity>N</severity>".into());
        pattern
            .contains
            .get_mut("/messages")
            .expect("inserted above")
            .push(SEVERITY_TWO.into());
        pattern
            .excludes
            .insert("/messages".into(), vec![STAGE_ONE.into()]);
    }
    pattern
}
fn stage(
    model: &str,
    protocol: &str,
    number: u8,
    script: &str,
    status: u16,
) -> NativeClassifierStage {
    NativeClassifierStage {
        identity: format!("{protocol}.stage{number}"),
        model: model.into(),
        request: signature(protocol, number),
        script: script.into(),
        status,
    }
}
fn case(
    name: &str,
    model: &str,
    allowed: &[&str],
    preferred: Option<&str>,
    outcome: Outcome,
    stages: Vec<NativeClassifierStage>,
) -> NativeClassifierCase {
    NativeClassifierCase {
        name: name.into(),
        session_model: model.into(),
        available_models: allowed.iter().map(|m| (*m).into()).collect(),
        profile_env: preferred
            .map(|model| BTreeMap::from([("ANTHROPIC_DEFAULT_SONNET_MODEL".into(), model.into())]))
            .unwrap_or_default(),
        declared_reviewer: preferred
            .filter(|model| allowed.contains(model))
            .unwrap_or(model)
            .into(),
        outcome,
        stages,
        initial_request: main_pattern(false),
        continuation_request: main_pattern(true),
    }
}
pub(crate) fn recipe() -> NativeClassifierReview {
    let unavailable = "error permission status=403 controlled-native-classifier-unavailable";
    NativeClassifierReview {
        command:r#"node -e 'require("node:fs").writeFileSync(process.argv[1], "native-auto-probe\n", {flag:"wx"})' __ORCH_MARKER__"#.into(),
        marker_contents:"native-auto-probe\n".into(),
        command_arguments:json!({"description":"Create only the disposable sibling scratch marker"}),
        sandbox_mode:SandboxMode::Open,excluded_refusal_fragment:"availableModels".into(),
        cases:vec![
            case("severity-allow",SONNET,&[SONNET],None,Outcome::Allow,vec![stage(SONNET,"severity",1,"say <severity>0</severity>",200)]),
            case("severity-deny",SONNET,&[SONNET],None,Outcome::Deny,vec![stage(SONNET,"severity",1,"say <severity>100</severity>",200),
                stage(SONNET,"severity",2,"say <thinking>Controlled fixture decision.</thinking><severity>100</severity>",200)]),
            case("severity-unavailable",SONNET,&[SONNET],None,Outcome::Unavailable,vec![stage(SONNET,"severity",1,unavailable,403)]),
            case("basic-session-only",OPUS,&[OPUS],None,Outcome::Unavailable,vec![stage(OPUS,"block",1,unavailable,403)]),
            case("basic-default-both",OPUS,&[OPUS,SONNET],None,Outcome::Unavailable,vec![stage(OPUS,"block",1,unavailable,403)]),
            case("profile-allowed-fallback",OPUS,&[OPUS,SONNET],Some(SONNET),Outcome::Unavailable,
                vec![stage(SONNET,"severity",1,unavailable,403),stage(OPUS,"severity",1,unavailable,403)]),
            case("profile-excluded-preference",OPUS,&[OPUS],Some(SONNET),Outcome::Unavailable,vec![stage(OPUS,"block",1,unavailable,403)]),
        ],
    }
}

pub(crate) fn verify(e: &NativeClassifierEvidence<'_>) -> Result<(), String> {
    let fail = |text: &str| Err(format!("{}: {text}", e.case.name));
    let requests: Vec<_> = e
        .requests
        .iter()
        .filter(|request| request.path.ends_with("/v1/messages"))
        .collect();
    let count = if e.control {
        1
    } else {
        2 + e.case.stages.len()
    };
    if requests.len() != count {
        let observed: Vec<_> = requests.iter().map(|request| json!({
            "model": request.body["model"].as_str(), "status": request.status,
            "queued": request.queued,
            "initial": e.case.initial_request.matches(request.body),
            "continuation": e.case.continuation_request.matches(request.body),
            "stages": e.case.stages.iter().filter(|stage| stage.request.matches(request.body))
                .map(|stage| stage.identity.as_str()).collect::<Vec<_>>(),
        })).collect();
        return Err(format!(
            "{}: unexpected model request count (control={}, expected={count}, observed={observed:?}); no response is attributed by order alone",
            e.case.name, e.control
        ));
    }
    if requests.iter().any(|request| {
        !request.queued
            || request.body["model"].as_str().is_none_or(|model| {
                !e.case
                    .available_models
                    .iter()
                    .any(|allowed| allowed == model)
            })
    }) {
        return fail("unallowed model or non-queued response observed");
    }
    if requests[0].body["model"] != e.case.session_model
        || requests[0].status != Some(200)
        || !e.case.initial_request.matches(requests[0].body)
    {
        return fail("main tool-producing request did not match its actual structure");
    }
    if !e.control {
        for (index, stage) in e.case.stages.iter().enumerate() {
            let actual = requests[index + 1];
            if actual.body["model"] != stage.model
                || actual.status != Some(stage.status)
                || !stage.request.matches(actual.body)
            {
                return fail(
                    "classifier protocol/model/stage structure differs from the observed recipe",
                );
            }
        }
        let last = requests.last().ok_or("main continuation absent")?;
        if last.body["model"] != e.case.session_model
            || last.status != Some(200)
            || !e.case.continuation_request.matches(last.body)
        {
            return fail("main continuation did not match its structural response selector");
        }
    }
    let raw: Vec<_> = e
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::RawFrame(frame) if frame.direction == crate::events::FROM_HARNESS => {
                Some(&frame.payload)
            }
            _ => None,
        })
        .collect();
    let tools: Vec<_> = raw
        .iter()
        .filter(|frame| frame["type"] == "assistant")
        .filter_map(|frame| frame["message"]["content"].as_array())
        .flatten()
        .filter(|block| {
            block["type"] == "tool_use"
                && block["name"] == "Bash"
                && block["input"]["command"] == e.command
        })
        .filter_map(|block| block["id"].as_str())
        .collect();
    if tools.len() != 1 {
        return fail("one exact native Bash invocation was not observed");
    }
    let reviews: Vec<_> = e
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::ItemCompleted(item)
                if item.payload.pointer("/tool/name") == Some(&json!(REVIEW_TOOL)) =>
            {
                Some(item)
            }
            _ => None,
        })
        .collect();
    let asks = e
        .events
        .iter()
        .filter(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
        .count();
    if e.control {
        let exact=e.events.iter().any(|event| match event {
            SessionEvent::ApprovalRequested(asked) if asked.turn_id==e.turn_id =>
                matches!(&asked.request.detail, orch_agent_contract::events::HarnessRequestDetail::Command {command,..} if command==e.command),
            _ => false,
        });
        if asks != 1 || !exact || !reviews.is_empty() {
            return fail(
                "On request did not preserve the exact unanswered native permission request",
            );
        }
        return Ok(());
    }
    if asks != 0 {
        return fail("Auto left a person request; it was not answered");
    }
    if e.case.outcome == Outcome::Allow {
        // Native success is classifier traffic plus the independently checked effect. There is no
        // affirmative review hook in this pin, so producing a review item here would be fabrication.
        if !reviews.is_empty() {
            return fail("allow unexpectedly produced a denial or fabricated review item");
        }
        return Ok(());
    }
    if reviews.len() != 1 {
        return fail("one factual denial hook and one mapped review item are required");
    }
    // Incoming control frames are handled before the ordinary raw-frame mapper. The existing
    // review item retains the actual PermissionDenied input in RawCall.output; use that native
    // evidence rather than adding fabricated control notifications or changing runtime reporting.
    let output = reviews[0]
        .payload
        .pointer("/raw/output/inline")
        .and_then(Json::as_str)
        .ok_or("native denial hook input is absent from raw review output")?;
    let input: Json = serde_json::from_str(output)
        .map_err(|error| format!("native denial hook input is incomplete: {error}"))?;
    if input["hook_event_name"] != "PermissionDenied"
        || input["permission_mode"] != "auto"
        || input["session_id"] != e.native_session_ref
        || input["tool_name"] != "Bash"
        || input["tool_input"]["command"] != e.command
        || input["tool_use_id"] != tools[0]
    {
        return fail("native denial lost session/tool-ID/command correlation");
    }
    let expected = if e.case.outcome == Outcome::Deny {
        "denied"
    } else {
        "failed"
    };
    if (e.case.outcome == Outcome::Deny && input["reason"] != "Blocked by classifier")
        || (e.case.outcome == Outcome::Unavailable && input["reason"] != "Classifier unavailable")
    {
        return fail("native denial kind differs from the controlled classification outcome");
    }
    if input.get("usage").is_some_and(|usage| !usage.is_null()) {
        return fail("new native separate-review usage requires explicit projection support");
    }
    let reviewer = e
        .reviewer
        .ok_or("active process reviewer declaration absent")?;
    let native_model = input.get("model").and_then(Json::as_str);
    let result = reviews[0]
        .payload
        .pointer("/tool/result")
        .ok_or("review item result absent")?;
    if result["outcome"] != expected
        || result["reason"] != input["reason"]
        || result["reported"] != json!(native_model.is_some())
        || result["model"] != native_model.unwrap_or(&reviewer.model)
        || !result["usage"].is_null()
    {
        return fail(
            "review projection changed native outcome/model provenance or invented separate usage",
        );
    }
    if !e.events.iter().any(|event| {
        matches!(event,SessionEvent::ItemStarted(start)
        if start.native_ref==reviews[0].native_ref && start.turn_id==e.turn_id)
    }) {
        return fail("review timeline item lost current-turn correlation");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_name_and_order_cannot_select_a_classifier_response() {
        let request = json!({"model":SONNET,"thinking":{"type":"disabled"},"max_tokens":64,"stop_sequences":["</severity>"],
            "system":format!("{MONITOR} Output <severity>N</severity>"),
            "messages":[{"role":"user","content":format!("<transcript>marker</transcript> {STAGE_ONE} Respond with <severity>N</severity> ONLY")}]});
        assert!(signature("severity", 1).matches(&request));
        assert!(!signature("severity", 2).matches(&request));
        assert!(!signature("block", 1).matches(&request));
        let mut main = request.clone();
        main["stream"] = json!(true);
        main["tools"] = json!([]);
        assert!(!signature("severity", 1).matches(&main));
        let mut unknown = request;
        unknown["stop_sequences"] = json!(["</unknown>"]);
        assert!(!signature("severity", 1).matches(&unknown));
    }
}
