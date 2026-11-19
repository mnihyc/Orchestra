//! The addresses the connector serves a harness on, beside it.
//!
//! A connector in the `agent` role runs one HTTP listener on `127.0.0.1` at a random port for the harness
//! processes it hosts. Everything a harness reaches beyond itself goes through it, under the key of its
//! session:
//!
//! | Path | Purpose |
//! |------|---------|
//! | `/llm/<session key>/…` | Model calls of a forwarded session |
//! | `/exec/<session key>` (WebSocket) | The harness's connection to its executor |
//! | `/mcp/<session key>/<server name>` | An environment-hosted MCP server; the name `exec` is Orch's executor |
//! | `/mcp/<session key>/orch` | Orch's own tools |
//!
//! The session key is 128 random bits for each session ([`is_session_key`]), and the listener also requires
//! `Authorization: Bearer <token>`, with 256 random bits for each harness process, which the harness gets
//! through an environment variable and never as an argument or in a file readable by others. The listener
//! rejects any `Host` other than `127.0.0.1:<port>` ([`host_is_allowed`]).
//!
//! The adapter is given the base address and the token when its harness is started ([`Loopback`]) and the
//! key of each session when the session is opened; this module builds the addresses from them, and the
//! listener reads them back with [`parse_path`].

use orch_plugin_contract::Secret;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The address the listener is bound to: the machine itself and nothing else.
pub const LOOPBACK_HOST: &str = "127.0.0.1";
/// The name of the MCP server that is Orch's own tools.
pub const ORCH_SERVER: &str = "orch";
/// The name of the MCP server that is Orch's executor, in the tool set the environment offers.
pub const EXEC_SERVER: &str = "exec";
/// How many hexadecimal digits a session key has: 128 bits.
pub const SESSION_KEY_DIGITS: usize = 32;

/// Where the listener is and what a harness has to present to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Loopback {
    /// The listener's base address: `http://127.0.0.1:<port>`.
    pub base_url: String,
    /// The bearer token of this harness process: 256 random bits.
    pub token: Secret,
}

impl Loopback {
    /// The listener on `port`, with `token`.
    #[must_use]
    pub fn new(port: u16, token: Secret) -> Self {
        Self {
            base_url: format!("http://{LOOPBACK_HOST}:{port}"),
            token,
        }
    }

    /// The model endpoint of a session; a protocol's own paths follow it (`/v1/responses`).
    #[must_use]
    pub fn llm(&self, session_key: &str) -> String {
        format!("{}/llm/{session_key}", self.base_url)
    }

    /// The WebSocket address of a session's connection to its executor.
    #[must_use]
    pub fn exec(&self, session_key: &str) -> String {
        format!(
            "{}/exec/{session_key}",
            self.base_url.replacen("http://", "ws://", 1)
        )
    }

    /// The address of the MCP server `name` of a session.
    #[must_use]
    pub fn mcp(&self, session_key: &str, name: &str) -> String {
        format!("{}/mcp/{session_key}/{name}", self.base_url)
    }

    /// The address of Orch's own tools for a session.
    #[must_use]
    pub fn orch(&self, session_key: &str) -> String {
        self.mcp(session_key, ORCH_SERVER)
    }
}

/// Whether `text` is a session key: 32 lowercase hexadecimal digits.
#[must_use]
pub fn is_session_key(text: &str) -> bool {
    text.len() == SESSION_KEY_DIGITS && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Whether a `Host` header is the listener's own: `127.0.0.1:<port>` and nothing else.
#[must_use]
pub fn host_is_allowed(host: &str, port: u16) -> bool {
    host == format!("{LOOPBACK_HOST}:{port}")
}

/// What a path of the listener asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// A model call; `rest` is the path below the session, such as `/v1/responses`.
    Llm {
        /// The session.
        session_key: String,
        /// What follows the session key, with its leading `/`; empty for none.
        rest: String,
    },
    /// The connection to the session's executor.
    Exec {
        /// The session.
        session_key: String,
    },
    /// An MCP server.
    Mcp {
        /// The session.
        session_key: String,
        /// The server's name; [`ORCH_SERVER`] is Orch's own tools.
        server: String,
    },
}

/// Reads a request path of the listener. `None` for anything that is not one of the four forms, or whose
/// key is not a session key.
#[must_use]
pub fn parse_path(path: &str) -> Option<Route> {
    let rest = path.strip_prefix('/')?;
    let (area, tail) = rest.split_once('/')?;
    let (key, below) = match tail.split_once('/') {
        Some((key, below)) => (key, Some(below)),
        None => (tail, None),
    };
    if !is_session_key(key) {
        return None;
    }
    let session_key = key.to_owned();
    match (area, below) {
        ("llm", below) => Some(Route::Llm {
            session_key,
            rest: below.map_or_else(String::new, |below| format!("/{below}")),
        }),
        ("exec", None) => Some(Route::Exec { session_key }),
        ("mcp", Some(server)) if !server.is_empty() && !server.contains('/') => Some(Route::Mcp {
            session_key,
            server: server.to_owned(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn loopback() -> Loopback {
        Loopback::new(41231, Secret::new("t".repeat(64)))
    }

    #[test]
    fn the_addresses_of_a_session_are_built_under_its_key() {
        let at = loopback();
        assert_eq!(at.base_url, "http://127.0.0.1:41231");
        assert_eq!(at.llm(KEY), format!("http://127.0.0.1:41231/llm/{KEY}"));
        assert_eq!(at.exec(KEY), format!("ws://127.0.0.1:41231/exec/{KEY}"));
        assert_eq!(
            at.mcp(KEY, "tmux"),
            format!("http://127.0.0.1:41231/mcp/{KEY}/tmux")
        );
        assert_eq!(
            at.orch(KEY),
            format!("http://127.0.0.1:41231/mcp/{KEY}/orch")
        );
        assert!(
            !format!("{at:?}").contains("tttt"),
            "the token is never printed"
        );
    }

    #[test]
    fn a_path_is_read_back_into_what_it_asks_for() {
        assert_eq!(
            parse_path(&format!("/llm/{KEY}/v1/responses")),
            Some(Route::Llm {
                session_key: KEY.into(),
                rest: "/v1/responses".into()
            })
        );
        assert_eq!(
            parse_path(&format!("/llm/{KEY}")),
            Some(Route::Llm {
                session_key: KEY.into(),
                rest: String::new()
            })
        );
        assert_eq!(
            parse_path(&format!("/exec/{KEY}")),
            Some(Route::Exec {
                session_key: KEY.into()
            })
        );
        assert_eq!(
            parse_path(&format!("/mcp/{KEY}/orch")),
            Some(Route::Mcp {
                session_key: KEY.into(),
                server: ORCH_SERVER.into()
            })
        );
        assert_eq!(parse_path(&format!("/exec/{KEY}/more")), None);
        assert_eq!(parse_path(&format!("/mcp/{KEY}")), None);
        assert_eq!(parse_path(&format!("/mcp/{KEY}/a/b")), None);
        assert_eq!(parse_path("/llm/short/v1/responses"), None);
        assert_eq!(parse_path(&format!("/files/{KEY}")), None);
        assert_eq!(parse_path("/"), None);
    }

    #[test]
    fn a_session_key_is_128_bits_of_lowercase_hex_and_the_host_is_the_listeners_own() {
        assert!(is_session_key(KEY));
        assert!(!is_session_key(&KEY.to_uppercase()));
        assert!(!is_session_key(&KEY[1..]));
        assert!(host_is_allowed("127.0.0.1:41231", 41231));
        assert!(!host_is_allowed("localhost:41231", 41231));
        assert!(!host_is_allowed("127.0.0.1:41232", 41231));
        assert!(!host_is_allowed("127.0.0.1", 41231));
    }
}
