//! A configured local service is attached without starting a child, and an incompatible reply is refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_config::ServerConfig;
use orch_plugin::transport;
use orch_plugin::{Options, Plugin};
use orch_plugin_contract::{
    PluginState,
    registration::{ConnectAddress, PluginStart},
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

async fn listener() -> tokio::net::TcpListener {
    for port in 41000..=41999 {
        if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            return listener;
        }
    }
    panic!("the test's reserved local port range has no free address");
}

#[tokio::test]
async fn a_tcp_attachment_has_no_child_process_or_captured_child_log() {
    let listener = listener().await;
    let port = listener.local_addr().unwrap().port();
    let link = transport::open(&PluginStart::Attach(ConnectAddress::Tcp(port)))
        .await
        .unwrap();
    assert!(link.process.is_none());
    assert!(link.log.is_none());
    let _ = listener.accept().await.unwrap();
}

#[tokio::test]
async fn the_servers_attach_configuration_negotiates_or_refuses_the_services_actual_contract() {
    for compatible in [true, false] {
        let listener = listener().await;
        let port = listener.local_addr().unwrap().port();
        let peer = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            while let Some(line) = lines.next_line().await.unwrap() {
                let request: Value = serde_json::from_str(&line).unwrap();
                let Some(id) = request.get("id") else {
                    continue;
                };
                let result = if request["method"] == "hello" {
                    json!({"kind":"summarizer","name":"local","version":"1","transport":if compatible {1}else{2},"contract":1,
                        "methods":["describe","summarize"],"descriptor":{"name":"local","title":"Local summarizer","config_schema":{},"fields":["description"]}})
                } else {
                    json!({})
                };
                if write
                    .write_all(format!("{}\n", json!({"id":id,"result":result})).as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
        });
        let config: ServerConfig = orch_config::load_text(
            &format!(
                "[[plugin]]\nkind='summarizer'\nname='local'\nconnect='tcp://127.0.0.1:{port}'\n"
            ),
            "attach-check",
            |_| None,
        )
        .unwrap();
        let registration = config.plugin[0].registration().unwrap();
        assert!(registration.command.is_none());
        let plugin = Plugin::start(&registration, Options::new("0.1.0"))
            .await
            .unwrap();
        plugin.settled().await;
        assert_eq!(
            plugin.report().state,
            if compatible {
                PluginState::Running
            } else {
                PluginState::Failed
            }
        );
        if !compatible {
            assert!(plugin.report().detail.is_some());
        }
        plugin.shutdown().await;
        peer.abort();
    }
}
