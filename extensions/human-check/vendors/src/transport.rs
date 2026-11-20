//! The one way a backend calls a vendor.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use orch_human_check_contract::edge::{EdgeAnswer, EdgeReader};
use orch_human_check_contract::{Unreachable, Why};

/// The largest answer read from a vendor: a mebibyte, far more than any of their answers, and a bound on what a broken
/// vendor can make this server hold.
pub const MAX_ANSWER_BYTES: usize = 1024 * 1024;

/// What a call carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Form fields, as `application/x-www-form-urlencoded`.
    Form(Vec<(String, String)>),
    /// A JSON document, as text.
    Json(String),
}

/// What a vendor answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The HTTP status.
    pub status: u16,
    /// The body, at most [`MAX_ANSWER_BYTES`].
    pub body: Vec<u8>,
}

/// A way to POST to a vendor. An implementation must not put the address it called, or the body, into the error it
/// returns: the address of one vendor carries a key.
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    /// Posts `body` to `url`, giving up after `timeout`.
    async fn post(&self, url: &str, body: Body, timeout: Duration) -> Result<Reply, Unreachable>;
}

/// What a failed call to a vendor, or to the web edge, was: a word and no address.
fn failed(error: &reqwest::Error) -> Unreachable {
    if error.is_timeout() {
        Unreachable::new(Why::Timeout, "the vendor did not answer in time")
    } else if error.is_connect() {
        Unreachable::new(Why::Connect, "the vendor could not be reached")
    } else {
        Unreachable::new(Why::Connect, "the call to the vendor failed")
    }
}

/// The transport that reaches a vendor over HTTPS.
#[derive(Debug, Clone)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    /// A client of its own, which follows no redirect: a vendor answers at the address it publishes.
    pub fn new() -> Result<Self, Unreachable> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Unreachable::new(Why::Setup, "the HTTP client could not be made"))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl Transport for Http {
    async fn post(&self, url: &str, body: Body, timeout: Duration) -> Result<Reply, Unreachable> {
        let request = self.client.post(url).timeout(timeout);
        let request = match body {
            Body::Form(fields) => request
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded",
                )
                .body(
                    url::form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(
                            fields
                                .iter()
                                .map(|(name, value)| (name.as_str(), value.as_str())),
                        )
                        .finish(),
                ),
            Body::Json(text) => request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(text),
        };
        let mut response = request.send().await.map_err(|error| failed(&error))?;
        let status = response.status().as_u16();
        let limit = u64::try_from(MAX_ANSWER_BYTES).unwrap_or(u64::MAX);
        let too_large = || Unreachable::new(Why::Unreadable, "the answer is too large");
        if response
            .content_length()
            .is_some_and(|length| length > limit)
        {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| failed(&error))? {
            if bytes.len().saturating_add(chunk.len()) > MAX_ANSWER_BYTES {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Reply {
            status,
            body: bytes,
        })
    }
}

#[async_trait]
impl EdgeReader for Http {
    async fn read(&self, url: &str, timeout: Duration) -> Result<EdgeAnswer, Unreachable> {
        let response = self
            .client
            .get(url)
            .timeout(timeout)
            .header(reqwest::header::USER_AGENT, "Orch-edge-look")
            .send()
            .await
            .map_err(|error| failed(&error))?;
        let policies = response
            .headers()
            .get_all(reqwest::header::CONTENT_SECURITY_POLICY)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::to_owned)
            .collect();
        Ok(EdgeAnswer {
            status: response.status().as_u16(),
            policies,
        })
    }
}

/// A call that was made, as a scripted transport saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    /// Where.
    pub url: String,
    /// What.
    pub body: Body,
    /// How long it was allowed.
    pub timeout: Duration,
}

/// A transport that answers what it was told to, one answer to each call, and keeps the calls it saw: the vendor of
/// the tests.
#[derive(Debug, Default)]
pub struct Scripted {
    answers: Mutex<VecDeque<Result<Reply, Unreachable>>>,
    calls: Mutex<Vec<Call>>,
}

impl Scripted {
    /// A vendor that answers `answers`, in order.
    #[must_use]
    pub fn new(answers: Vec<Result<Reply, Unreachable>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// A vendor that answers `status` and `body` once.
    #[must_use]
    pub fn saying(status: u16, body: &str) -> Self {
        Self::new(vec![Ok(Reply {
            status,
            body: body.as_bytes().to_vec(),
        })])
    }

    /// Adds one more answer.
    pub fn then(&self, answer: Result<Reply, Unreachable>) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.push_back(answer);
        }
    }

    /// Forgets the answers not given and the calls seen, for a test that starts again.
    pub fn clear(&self) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.clear();
        }
        if let Ok(mut calls) = self.calls.lock() {
            calls.clear();
        }
    }

    /// The calls seen so far.
    #[must_use]
    pub fn calls(&self) -> Vec<Call> {
        self.calls
            .lock()
            .map(|calls| calls.clone())
            .unwrap_or_default()
    }
}

#[async_trait]
impl Transport for Scripted {
    async fn post(&self, url: &str, body: Body, timeout: Duration) -> Result<Reply, Unreachable> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(Call {
                url: url.to_owned(),
                body,
                timeout,
            });
        }
        self.answers
            .lock()
            .ok()
            .and_then(|mut answers| answers.pop_front())
            .unwrap_or_else(|| Err(Unreachable::new(Why::Connect, "nothing was scripted")))
    }
}
