//! The web edge's side of the human check: the policy line a service needs, and a scripted edge for the tests.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use orch_human_check_contract::edge::{EdgeAnswer, EdgeReader};
use orch_human_check_contract::{Unreachable, Why};

use crate::edge_hosts_of;

/// The content policy the web edge sends where a human check of `kind` is chosen, as one directive of the edge's
/// configuration: what `deploy/frontend-csp.sh` makes of `ORCH_FRONTEND_HUMAN_CHECK`, written out whole, for an edge
/// whose configuration is kept from an older release and so does not read that variable. None where the kind is not
/// one this build knows.
#[must_use]
pub fn policy_of(kind: &str, alternate: bool) -> Option<String> {
    let hosts = edge_hosts_of(kind, alternate)?;
    let list = |hosts: &[&str]| -> String {
        hosts
            .iter()
            .map(|host| format!(" {host}"))
            .collect::<String>()
    };
    let frame = if hosts.frame.is_empty() {
        String::new()
    } else {
        format!("; frame-src 'self'{}", list(hosts.frame))
    };
    Some(format!(
        "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'nonce-$request_id' 'wasm-unsafe-eval'{}; style-src 'self' 'unsafe-inline'{}; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self'{}; worker-src 'self'; manifest-src 'self'{frame}",
        list(hosts.script),
        list(hosts.style),
        list(hosts.connect),
    ))
}

/// The line of the edge's `nginx.conf` that sends the policy of [`policy_of`].
#[must_use]
pub fn nginx_line_of(kind: &str, alternate: bool) -> Option<String> {
    policy_of(kind, alternate)
        .map(|policy| format!("add_header Content-Security-Policy \"{policy}\" always;"))
}

/// A web edge that answers what it was told to for each address, and keeps the addresses it was asked for: the edge
/// of the tests.
#[derive(Debug, Default)]
pub struct ScriptedEdge {
    answers: Mutex<BTreeMap<String, Result<EdgeAnswer, Unreachable>>>,
    asked: Mutex<Vec<String>>,
}

impl ScriptedEdge {
    /// An edge that answers nothing: every address cannot be reached.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes the edge answer `policies` with status 200 at `url`.
    pub fn serving(&self, url: &str, policies: &[impl AsRef<str>]) {
        self.say(
            url,
            Ok(EdgeAnswer {
                status: 200,
                policies: policies
                    .iter()
                    .map(|policy| policy.as_ref().to_owned())
                    .collect(),
            }),
        );
    }

    /// Makes the edge answer `answer` at `url`.
    pub fn say(&self, url: &str, answer: Result<EdgeAnswer, Unreachable>) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.insert(url.to_owned(), answer);
        }
    }

    /// Forgets every answer and every address asked for.
    pub fn clear(&self) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.clear();
        }
        if let Ok(mut asked) = self.asked.lock() {
            asked.clear();
        }
    }

    /// The addresses asked for so far, in order.
    #[must_use]
    pub fn asked(&self) -> Vec<String> {
        self.asked
            .lock()
            .map(|asked| asked.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl EdgeReader for ScriptedEdge {
    async fn read(&self, url: &str, _timeout: Duration) -> Result<EdgeAnswer, Unreachable> {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(url.to_owned());
        }
        self.answers
            .lock()
            .ok()
            .and_then(|answers| answers.get(url).cloned())
            .unwrap_or_else(|| Err(Unreachable::new(Why::Connect, "nothing was scripted")))
    }
}
