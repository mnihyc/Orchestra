//! Read the environment-local ports endpoint without a person's token or a connector data directory.

use crate::arguments::{ForwardTransport, Ports};
use orch_plugin_contract::Secret;

fn path(command: &Ports) -> String {
    match command {
        Ports::Address { port } => format!("/ports/{port}"),
        Ports::List {
            look,
            port,
            limit,
            cursor,
        } => {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            if *look {
                query.append_pair("look", "true");
            }
            if let Some(port) = port {
                query.append_pair("port", &port.to_string());
            }
            if let Some(limit) = limit {
                query.append_pair("limit", &limit.to_string());
            }
            if let Some(cursor) = cursor {
                query.append_pair("cursor", cursor);
            }
            let query = query.finish();
            if query.is_empty() {
                "/ports".into()
            } else {
                format!("/ports?{query}")
            }
        }
        Ports::Whois {
            peer,
            listener_port,
            transport,
        } => {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            query
                .append_pair("peer", &peer.to_string())
                .append_pair("listener_port", &listener_port.to_string())
                .append_pair(
                    "transport",
                    match transport {
                        ForwardTransport::Tcp => "tcp",
                        ForwardTransport::Udp => "udp",
                    },
                );
            format!("/whois?{}", query.finish())
        }
    }
}

/// Print exactly the canonical JSON data; a definite nonmatch is the JSON value null.
pub async fn run(command: Ports) -> anyhow::Result<()> {
    let base = std::env::var("ORCH_PORTS_URL").map_err(|_| {
        anyhow::anyhow!(
            "ORCH_PORTS_URL is not set: run this command inside an attached environment"
        )
    })?;
    let token = std::env::var("ORCH_PORTS_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .map(Secret::new);
    let data = orch_connector_core::local_ports::read_local(&base, token.as_ref(), &path(&command))
        .await?;
    println!("{}", serde_json::to_string_pretty(&data)?);
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;

    #[test]
    fn a_read_keeps_the_exact_peer_and_cursor_without_turning_them_into_a_path() {
        let value = path(&Ports::Whois {
            peer: "[::1]:43123".parse().unwrap(),
            listener_port: 8080,
            transport: ForwardTransport::Udp,
        });
        let query = url::form_urlencoded::parse(value.split_once('?').unwrap().1.as_bytes())
            .into_owned()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(query["peer"], "[::1]:43123");
        assert_eq!(query["listener_port"], "8080");
        assert_eq!(query["transport"], "udp");
        let value = path(&Ports::List {
            look: true,
            port: None,
            limit: Some(20),
            cursor: Some("a&port=1/#x".into()),
        });
        let query = url::form_urlencoded::parse(value.split_once('?').unwrap().1.as_bytes())
            .into_owned()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(query["cursor"], "a&port=1/#x");
        assert!(!query.contains_key("port"));
        assert_eq!(path(&Ports::Address { port: 8080 }), "/ports/8080");
    }
}
