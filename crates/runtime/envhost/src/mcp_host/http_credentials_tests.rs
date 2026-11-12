use super::*;

#[test]
fn native_http_headers_resolve_only_their_named_variables_with_native_precedence() {
    let definition: McpDefinition = serde_json::from_value(json!({
        "spec":{"transport":"http","url":"https://example.test/mcp"},
        "http_credentials":{
            "headers":{"X-Static":"static-private","X-Override":"old","X-Empty":"kept","Authorization":"old-auth"},
            "env_headers":{"X-Override":"WORKSPACE","X-Empty":"EMPTY","X-Missing":"MISSING"},
            "bearer_token_env_var":"SERVICE_TOKEN"
        }
    })).unwrap();
    let variables = BTreeMap::from([
        ("WORKSPACE", "workspace-private"),
        ("EMPTY", " "),
        ("SERVICE_TOKEN", "token-private"),
    ]);
    let headers = private_http_headers(&definition, &|name| {
        variables.get(name).map(|v| (*v).to_owned())
    })
    .unwrap();
    assert_eq!(headers["x-static"], "static-private");
    assert_eq!(headers["x-override"], "workspace-private");
    assert_eq!(headers["x-empty"], "kept");
    assert!(!headers.contains_key("x-missing"));
    assert_eq!(headers["authorization"], "Bearer token-private");
    for value in headers.values() {
        assert!(value.is_sensitive());
    }
    let debug = format!("{headers:?}");
    for secret in ["static-private", "workspace-private", "token-private"] {
        assert!(!debug.contains(secret));
    }
    assert!(matches!(
        private_http_headers(&definition, &|_| None),
        Err(HostError::NotFound(_))
    ));
}

#[test]
fn native_http_templates_preserve_defaults_empty_values_and_credential_exclusions() {
    let definition: McpDefinition = serde_json::from_value(json!({
        "spec":{"transport":"http","url":"${BASE:-https://default.test}/mcp"},
        "http_credentials":{"interpolate":true,"blocked_env":["MODEL_TOKEN"],
            "headers":{"Authorization":"Bearer ${SERVICE_TOKEN}","X-Empty":"${EMPTY:-fallback}","X-Unknown":"${UNKNOWN}","X-Blocked":"${MODEL_TOKEN:-must-not-be-sent}"}}
    })).unwrap();
    let lookup = |name: &str| {
        assert_ne!(
            name, "MODEL_TOKEN",
            "excluded credentials must never be read"
        );
        match name {
            "SERVICE_TOKEN" => Some("service-private".to_owned()),
            "EMPTY" => Some(String::new()),
            _ => None,
        }
    };
    let http = definition.http_credentials.as_ref().unwrap();
    assert_eq!(
        http_template(definition.spec.url.as_deref().unwrap(), http, &lookup),
        "https://default.test/mcp"
    );
    let headers = private_http_headers(&definition, &lookup).unwrap();
    assert_eq!(headers["authorization"], "Bearer service-private");
    assert_eq!(headers["x-empty"], "");
    assert_eq!(headers["x-unknown"], "${UNKNOWN}");
    assert_eq!(headers["x-blocked"], "");
}

#[test]
fn an_invalid_imported_header_never_puts_its_value_in_the_error() {
    let definition: McpDefinition = serde_json::from_value(json!({
        "spec":{"transport":"http","url":"https://example.test/mcp"},
        "http_credentials":{"headers":{"Authorization":"private-token\r\nInjected: bad"}}
    }))
    .unwrap();
    let error = private_http_headers(&definition, &|_| None).unwrap_err();
    assert!(!error.to_string().contains("private-token"));
    assert!(!format!("{error:?}").contains("Injected"));
}
