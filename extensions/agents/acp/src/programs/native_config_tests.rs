//! The serialized public carrier drives the existing managed launch plan, with no route reconstruction.
#![allow(clippy::unwrap_used, reason = "malformed fixture data fails the test")]

use super::{dsh, tests::variables};
use orch_agent_contract::session::NativeModelContext;
use orch_provider_contract::capabilities::Resolution;
use serde_json::{Value as Json, json};

fn context(model: &str, capabilities: Json) -> NativeModelContext {
    NativeModelContext {
        model: model.into(),
        parameters: Default::default(),
        capabilities,
        extensions: Default::default(),
    }
}

#[test]
fn serialized_native_flags_override_stored_intent_without_copying_private_control_data() {
    let program = dsh().unwrap();
    let model = "bound-model";
    for (native, stored, enabled) in [("disabled", "live", false), ("live", "service", true)] {
        let mode = if native == "disabled" {
            "service"
        } else {
            "live"
        };
        let resolution = Resolution {
            capability: "web_search".into(),
            mode: Some(mode.to_owned()),
            native_config: [("web_search".into(), json!(native))].into_iter().collect(),
            ..Default::default()
        };
        let mut captured =
            NativeModelContext::captured(model.into(), Default::default(), &[resolution]).unwrap();
        captured.capabilities[0]["native_config"]["env.ORCH_LLM_TOKEN"] =
            json!("synthetic-private-value");
        captured.capabilities[0]["native_config"]["model_provider"] = json!("unexpected");
        captured.capabilities.as_array_mut().unwrap().push(json!({
            "capability":"image_generation","native_config":{
                "web_search":if native == "live" { "disabled" } else { "live" },
                "env.ORCH_LLM_TOKEN":"synthetic-private-value","model_provider":"unexpected"
            }
        }));
        let carried: NativeModelContext =
            serde_json::from_value(serde_json::to_value(captured).unwrap()).unwrap();
        let mut values = variables(&[model]);
        values.set("protocol", "anthropic-messages");
        values.set("api", program.api_of("anthropic-messages").unwrap());
        values
            .select_web_search_for_model(
                &json!({"web_search":stored}),
                Some("disabled"),
                &program,
                model,
                Some(&carried),
            )
            .unwrap();
        let plan = program.plan(&values).unwrap();
        let patch = String::from_utf8(plan.files[0].content.clone()).unwrap();
        assert_eq!(patch.contains("search: true\n"), enabled);
        assert!(patch.contains("fetch: true\n"));
        assert!(!patch.contains("synthetic-private-value"));
        assert!(!plan.env.contains_key("model_provider"));
    }
}

#[test]
fn unavailable_malformed_other_capability_and_stale_model_results_never_reenable_stored_live() {
    let program = dsh().unwrap();
    let model = "bound-model";
    for capabilities in [
        json!([]),
        json!([{"capability":"web_search","mode":"live","reason":"current_authority_refused"}]),
        json!([{"capability":"web_search","route":{"native_value":"live"}}]),
        json!([{"capability":"image_generation","native_config":{"web_search":"live"}}]),
        json!([{"capability":"web_search","native_config":{"web_search":true}}]),
        json!([{"capability":"web_search","native_config":{"web_search":{"nested":"live"}}}]),
        json!([{"capability":"web_search","native_config":{"web_search":"service"}}]),
        json!([{"capability":"web_search","native_config":{"web_search":"cached"}}]),
        json!([{"capability":"web_search","native_config":{"web_search":"live"}},
               {"capability":"web_search","native_config":{"web_search":"disabled"}}]),
    ] {
        let carried = context(model, capabilities);
        let mut values = variables(&[model]);
        values.set("protocol", "anthropic-messages");
        values
            .select_web_search_for_model(
                &json!({"web_search":"live"}),
                Some("disabled"),
                &program,
                model,
                Some(&carried),
            )
            .unwrap();
        let patch =
            String::from_utf8(program.plan(&values).unwrap().files[0].content.clone()).unwrap();
        assert!(!patch.contains("search: true\n"));
        assert!(patch.contains("fetch: true\n"));
    }
    let stale = context(
        "another-model",
        json!([{"capability":"web_search","native_config":{"web_search":"live"}}]),
    );
    assert!(
        program
            .resolved_native(&stale, model, "web_search", "web_search")
            .is_none()
    );
    let mut values = variables(&[model]);
    values.set("protocol", "anthropic-messages");
    values
        .select_web_search_for_model(
            &json!({"web_search":"live"}),
            Some("disabled"),
            &program,
            model,
            Some(&stale),
        )
        .unwrap();
    assert!(
        !String::from_utf8(program.plan(&values).unwrap().files[0].content.clone())
            .unwrap()
            .contains("search: true\n")
    );
}

#[test]
fn legacy_callers_and_unreported_defaults_keep_the_existing_native_choice_semantics() {
    let program = dsh().unwrap();
    let model = "bound-model";
    let mut values = variables(&[model]);
    values.set("protocol", "anthropic-messages");
    values
        .select_web_search_for_model(
            &json!({"web_search":"live"}),
            Some("disabled"),
            &program,
            model,
            None,
        )
        .unwrap();
    assert!(
        String::from_utf8(program.plan(&values).unwrap().files[0].content.clone())
            .unwrap()
            .contains("search: true\n")
    );
    let unreported = context(
        model,
        json!([{"capability":"web_search","mode":null,"native_config":{}}]),
    );
    values
        .select_web_search_for_model(&json!({}), Some("live"), &program, model, Some(&unreported))
        .unwrap();
    assert!(
        String::from_utf8(program.plan(&values).unwrap().files[0].content.clone())
            .unwrap()
            .contains("search: true\n"),
        "a missing instruction does not invent a new program default"
    );
}

#[test]
fn page_fetch_service_disables_the_native_reader_while_search_service_alone_keeps_it() {
    let program = dsh().unwrap();
    let model = "bound-model";
    for (search, fetch, enabled) in [
        ("disabled", None, true),
        ("disabled", Some(false), false),
        ("disabled", Some(true), true),
        ("live", Some(false), false),
    ] {
        let mut rows = vec![Resolution {
            capability: "web_search".into(),
            mode: Some(if search == "live" { "live" } else { "service" }.to_owned()),
            native_config: [("web_search".into(), json!(search))].into_iter().collect(),
            ..Default::default()
        }];
        if let Some(fetch) = fetch {
            let mode = if fetch { "on" } else { "service" };
            rows.push(Resolution {
                capability: "page_fetch".into(),
                mode: Some(mode.to_owned()),
                native_config: [("page_fetch".into(), json!(fetch))].into_iter().collect(),
                ..Default::default()
            });
        }
        let captured =
            NativeModelContext::captured(model.into(), Default::default(), &rows).unwrap();
        let carried: NativeModelContext =
            serde_json::from_value(serde_json::to_value(captured).unwrap()).unwrap();
        for protocol in program.protocol_order() {
            let mut values = variables(&[model]);
            values.set("protocol", protocol.clone());
            values.set("api", program.api_of(&protocol).unwrap());
            values
                .select_web_search_for_model(
                    &json!({"web_search":"live"}),
                    Some("disabled"),
                    &program,
                    model,
                    Some(&carried),
                )
                .unwrap();
            values.select_page_fetch_for_model(Some("on"), &program, model, Some(&carried));
            let patch =
                String::from_utf8(program.plan(&values).unwrap().files[0].content.clone()).unwrap();
            assert_eq!(
                patch.contains("search: true\n"),
                search == "live" && protocol == "anthropic-messages"
            );
            assert_eq!(
                patch.contains("fetch: true\n"),
                enabled,
                "{protocol} {fetch:?}"
            );
            assert!(patch.contains(&format!("fetch: {enabled}\n")));
        }
    }
    let unrelated = context(
        model,
        json!([{"capability":"web_search","native_config":{"page_fetch":false}}]),
    );
    assert!(
        program
            .resolved_native(&unrelated, model, "page_fetch", "page_fetch")
            .is_none(),
        "search cannot supply the separate page reader's switch"
    );
    let mut values = variables(&[model]);
    values.set("protocol", "anthropic-messages");
    values.select_page_fetch_for_model(Some("on"), &program, model, None);
    assert!(
        String::from_utf8(program.plan(&values).unwrap().files[0].content.clone())
            .unwrap()
            .contains("fetch: true\n"),
        "older callers keep the existing independent reader default"
    );
}
