//! The actual forwarder keeps image generation inside its recorded model ceiling.
use super::*;

#[tokio::test]
async fn native_image_requests_need_the_open_binding_and_closed_bindings_make_no_model_call() {
    let interfaces = orch_provider_contract::capabilities::protocol_interfaces().unwrap();
    let binding = &interfaces[RESPONSES].native_tools["image_generation"];
    for enabled in [true, false] {
        let p = ProviderRig::new().await;
        p.start_responses_provider().await;
        let selected = if enabled {
            json!(binding)
        } else {
            serde_json::Value::Null
        };
        let lease = serde_json::from_value::<orch_link_contract::commands::LeaseOpen>(json!({
            "lease_id":new_id(),"provider_id":p.rig.provider,"thread_id":p.thread,
            "mode":"forwarded","protocol":RESPONSES,"allowed_models":[MODEL],
            "model_parameters":{(MODEL):{"native_tools":{(RESPONSES):{"image_generation":selected}}}}
        })).unwrap();
        p.rig.succeed(&p.provider_key(), &lease).await;
        let mut input = responses_request(MODEL, "A blue square");
        input["stream"] = json!(false);
        input["tools"] = json!([{"type":"image_generation"}]);
        let output = json!({"id":"fixture-image-request","model":MODEL,"status":"completed","output":[
            {"id":"fixture-image","type":"image_generation_call","status":"completed","result":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Wl6B9sAAAAASUVORK5CYII="},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"Generated the square"}]}],"usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}});
        if enabled {
            p.mock.queue(&format!("body 200 {output}")).unwrap();
        }
        let response = p.harness(p.thread).post("/v1/responses", &input).await;
        if enabled {
            assert_eq!(response.status, 200);
            assert_eq!(response.json(), output);
            let requests = p.model_requests();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].body["tools"], input["tools"]);
            assert_eq!(requests[0].credential.verdict, Verdict::Accepted);
            assert!(!response.json().to_string().contains(&p.key));
        } else {
            assert_eq!(response.status, 403);
            assert_eq!(response.json()["error"]["code"], "request_refused");
            assert!(p.model_requests().is_empty());
        }
    }
}
