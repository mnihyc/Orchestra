//! An imported copy's missing executable is published as availability, without a fabricated crash.
use super::*;
use orch_agent_contract::discovery::McpDefinition;
use orch_link_contract::commands::{McpImport, McpStart, McpStartLocal, McpStop, True};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_missing_imported_command_announces_unavailable_facts_even_when_start_returns_an_error() {
    let mut rig = rig_with(&[], |settings, _| {
        settings.mcp.import = true;
    })
    .await;
    let server = orch_types::new_id();
    let definition: McpDefinition = serde_json::from_value(serde_json::json!({
        "spec":{"transport":"stdio","command":[rig.root.join("missing-imported-program")]},"secrets":{}
    })).unwrap();
    rig.host
        .mcp_import(
            &McpImport {
                expected_fingerprint: Some(definition.definition_fingerprint()),
                stored: None,
                transfer: None,
                environment_id: Some(rig.env),
                server_id: server,
                source: "fake".into(),
                scope: "user".into(),
                name: "missing-tool".into(),
                directory: None,
            },
            &definition,
            None,
        )
        .unwrap();
    let error = rig
        .host
        .mcp_start(
            &McpStart::Local(McpStartLocal {
                environment_id: rig.env,
                name: "missing-tool".into(),
                server_id: server,
                local: True,
            }),
            &BTreeMap::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(&error, HostError::NotFound(_)));
    assert!(error.to_string().contains("could not be started"));
    let report = rig.report("the unavailable imported command", |report| {
        matches!(report, Report::Facts(facts) if facts.mcp_servers.iter().any(|state| state.server_id==Some(server) && state.state=="unavailable"))
    }).await;
    let Report::Facts(facts) = report else {
        panic!("the actual availability facts are reported");
    };
    let state = facts
        .mcp_servers
        .iter()
        .find(|state| state.server_id == Some(server))
        .unwrap();
    assert_eq!(state.state, "unavailable");
    assert!(
        state
            .reason
            .as_deref()
            .unwrap()
            .contains("missing-imported-program")
    );
    assert!(rig.host.mcp_failure(rig.env, "missing-tool").is_some());
    assert!(
        rig.host
            .mcp_endpoint(rig.env, "missing-tool")
            .await
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_imported_private_copy_authenticates_http_initialize_notifications_and_tool_calls() {
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use hyper::{Request, Response, body::Incoming};
    use hyper_util::rt::TokioIo;
    use std::convert::Infallible;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut rig = rig_with(&[], |settings, _| settings.mcp.import = true).await;
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&seen);
    let stop = tokio_util::sync::CancellationToken::new();
    let server_stop = stop.clone();
    let upstream = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let socket = tokio::select! {
                () = server_stop.cancelled() => break,
                accepted = listener.accept() => accepted.unwrap().0,
            };
            let count = Arc::clone(&count);
            let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                let count = Arc::clone(&count);
                async move {
                    assert_eq!(
                        request.headers()["authorization"],
                        "Bearer copy-private-token"
                    );
                    assert_eq!(request.headers()["x-api-key"], "copy-private-header");
                    assert_eq!(request.headers()["x-workspace"], "workspace-private-value");
                    let bytes = request.into_body().collect().await.unwrap().to_bytes();
                    let message: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    count.fetch_add(1, Ordering::SeqCst);
                    let result = match message["method"].as_str() {
                        Some("initialize") => {
                            serde_json::json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"private-copy","version":"1"}})
                        }
                        Some("tools/list") => serde_json::json!({"tools":[]}),
                        _ => serde_json::json!({}),
                    };
                    let reply =
                        serde_json::json!({"jsonrpc":"2.0","id":message["id"],"result":result});
                    let mut response = Response::new(Full::new(Bytes::from(reply.to_string())));
                    response.headers_mut().insert(
                        hyper::header::CONTENT_TYPE,
                        hyper::header::HeaderValue::from_static("application/json"),
                    );
                    Ok::<_, Infallible>(response)
                }
            });
            let connection_stop = server_stop.clone();
            connections.spawn(async move {
                tokio::select! {
                    () = connection_stop.cancelled() => {},
                    result = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service) => {result.unwrap();},
                }
            });
        }
        while let Some(result) = connections.join_next().await {
            result.unwrap();
        }
    });
    let server_id = orch_types::new_id();
    let definition: McpDefinition = serde_json::from_value(serde_json::json!({
        "spec":{"transport":"http","url":format!("http://{address}/mcp"),
            "env":{"WORKSPACE":"workspace-private-value"},"secret_env":["SERVICE_TOKEN"]},
        "secrets":{"SERVICE_TOKEN":"copy-private-token"},
        "http_credentials":{"headers":{"X-Api-Key":"copy-private-header"},"env_headers":{"X-Workspace":"WORKSPACE"},"bearer_token_env_var":"SERVICE_TOKEN"}
    })).unwrap();
    let imported = rig
        .host
        .mcp_import(
            &McpImport {
                expected_fingerprint: Some(definition.definition_fingerprint()),
                stored: None,
                transfer: None,
                environment_id: Some(rig.env),
                server_id,
                source: "fake".into(),
                scope: "user".into(),
                name: "private-copy".into(),
                directory: None,
            },
            &definition,
            None,
        )
        .unwrap();
    let public = serde_json::to_string(&imported).unwrap();
    assert!(!public.contains("copy-private-header"));
    assert!(!public.contains("copy-private-token"));
    drop(definition);
    rig.host
        .mcp_start(
            &McpStart::Local(McpStartLocal {
                environment_id: rig.env,
                name: "private-copy".into(),
                server_id,
                local: True,
            }),
            &BTreeMap::new(),
        )
        .await
        .unwrap();
    let endpoint = rig
        .host
        .mcp_endpoint(rig.env, "private-copy")
        .await
        .unwrap();
    let reply: serde_json::Value = reqwest::Client::new()
        .post(format!("http://{}/mcp", endpoint.address))
        .bearer_auth(&endpoint.token)
        .json(&serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(reply["result"]["tools"], serde_json::json!([]), "{reply}");
    assert_eq!(seen.load(Ordering::SeqCst), 3);
    let report = rig.report("the private HTTP server started", |report| matches!(report,Report::Facts(facts) if facts.mcp_servers.iter().any(|state|state.server_id==Some(server_id)&&state.state=="running"))).await;
    let Report::Facts(facts) = report else {
        panic!("the running server is announced as facts");
    };
    let public = serde_json::to_string(&facts).unwrap();
    assert!(!public.contains("copy-private-header"));
    assert!(!public.contains("copy-private-token"));
    rig.host
        .mcp_stop(&McpStop {
            environment_id: rig.env,
            name: "private-copy".into(),
        })
        .await
        .unwrap();
    stop.cancel();
    tokio::time::timeout(PATIENCE, upstream)
        .await
        .unwrap()
        .unwrap();
}
