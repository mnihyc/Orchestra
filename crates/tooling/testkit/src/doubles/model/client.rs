//! A client for a model interface of either format, for tests: sends a request and reads the answer whole, or piece by piece.
//!
//! It is plain HTTP on the loopback, never through a proxy. A request is described with the types of the
//! `http` crate, so that headers a protocol has made can be sent as they are. An answer read whole keeps the
//! pieces as they arrived, and says whether the body broke off before its end, which is how a test sees a cut
//! stream; the pieces put together are the body, and the server-sent events in it are read with the
//! parser of the model contract.

use std::time::Duration;

use bytes::{Bytes, BytesMut};
use http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use orch_model_contract::usage::{SseEvent, SseParser};
use serde_json::Value;

/// How long a request may take before the client gives up: a test that hangs fails in half a minute and
/// not never.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Why a request could not be made.
#[derive(Debug, thiserror::Error)]
#[error("the request failed: {0}")]
pub struct ClientError(String);

impl From<reqwest::Error> for ClientError {
    fn from(error: reqwest::Error) -> Self {
        Self(error.to_string())
    }
}

/// A request to make.
#[derive(Debug, Clone)]
pub struct ClientRequest {
    /// The method.
    pub method: Method,
    /// The path and query, below the base address.
    pub path: String,
    /// The headers.
    pub headers: HeaderMap,
    /// The body.
    pub body: Bytes,
}

impl ClientRequest {
    /// A request of `method` to `path`, with no headers and no body.
    #[must_use]
    pub fn new(method: Method, path: &str) -> Self {
        Self {
            method,
            path: path.to_owned(),
            headers: HeaderMap::new(),
            body: Bytes::new(),
        }
    }

    /// A `GET`.
    #[must_use]
    pub fn get(path: &str) -> Self {
        Self::new(Method::GET, path)
    }

    /// A `POST` of a JSON document.
    #[must_use]
    pub fn post_json(path: &str, body: &Value) -> Self {
        let mut request = Self::new(Method::POST, path);
        request.body = Bytes::from(body.to_string());
        request.headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        request
    }

    /// Adds a header; a name or value that cannot be one is left out.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        if let (Ok(name), Ok(value)) = (HeaderName::try_from(name), HeaderValue::from_str(value)) {
            self.headers.insert(name, value);
        }
        self
    }

    /// Presents `key` as a bearer.
    #[must_use]
    pub fn bearer(self, key: &str) -> Self {
        self.header("authorization", &format!("Bearer {key}"))
    }

    /// Presents `key` as `x-api-key`.
    #[must_use]
    pub fn x_api_key(self, key: &str) -> Self {
        self.header("x-api-key", key)
    }

    /// Marks the request as one of the Messages format.
    #[must_use]
    pub fn messages_version(self) -> Self {
        self.header("anthropic-version", "2023-06-01")
    }

    /// Sets the body.
    #[must_use]
    pub fn with_body(mut self, body: &[u8]) -> Self {
        self.body = Bytes::copy_from_slice(body);
        self
    }

    /// Replaces the headers by `headers`.
    #[must_use]
    pub fn with_headers(mut self, headers: HeaderMap) -> Self {
        self.headers = headers;
        self
    }
}

/// An answer read to its end, or to where the body broke.
#[derive(Debug, Clone)]
pub struct ClientReply {
    /// The status.
    pub status: StatusCode,
    /// The headers.
    pub headers: HeaderMap,
    /// The pieces of the body as they arrived.
    pub chunks: Vec<Bytes>,
    /// Why the body broke off before its end, if it did.
    pub broke: Option<String>,
}

impl ClientReply {
    /// The body: the pieces put together.
    #[must_use]
    pub fn body(&self) -> Bytes {
        let mut whole = BytesMut::new();
        for chunk in &self.chunks {
            whole.extend_from_slice(chunk);
        }
        whole.freeze()
    }

    /// The body as text.
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body()).into_owned()
    }

    /// The body as JSON, or `Null` when it is none.
    #[must_use]
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body()).unwrap_or(Value::Null)
    }

    /// The server-sent events of the body.
    #[must_use]
    pub fn events(&self) -> Vec<SseEvent> {
        SseParser::new().push(&self.body())
    }

    /// The value of a header.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name)?.to_str().ok()
    }
}

/// An answer whose head has arrived and whose body is being read piece by piece.
#[derive(Debug)]
pub struct OpenReply {
    /// The status.
    pub status: StatusCode,
    /// The headers.
    pub headers: HeaderMap,
    response: reqwest::Response,
}

impl OpenReply {
    /// The next piece of the body: `Ok(None)` at its end, an error where it broke.
    pub async fn next_chunk(&mut self) -> Result<Option<Bytes>, ClientError> {
        Ok(self.response.chunk().await?)
    }
}

/// A client of one base address.
#[derive(Debug, Clone)]
pub struct ModelClient {
    http: reqwest::Client,
    base: String,
}

impl ModelClient {
    /// A client of `base`, such as `http://127.0.0.1:41500`; the paths of requests are appended to it.
    #[must_use]
    pub fn new(base: &str) -> Self {
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
        }
    }

    fn builder(&self, request: &ClientRequest) -> reqwest::RequestBuilder {
        self.http
            .request(
                request.method.clone(),
                format!("{}{}", self.base, request.path),
            )
            .headers(request.headers.clone())
            .body(request.body.clone())
    }

    /// Sends a request and reads the answer to its end, or to where the body breaks.
    pub async fn send(&self, request: ClientRequest) -> Result<ClientReply, ClientError> {
        let mut response = self.builder(&request).send().await?;
        let (status, headers) = (response.status(), response.headers().clone());
        let mut chunks = Vec::new();
        let mut broke = None;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => chunks.push(chunk),
                Ok(None) => break,
                Err(error) => {
                    broke = Some(error.to_string());
                    break;
                }
            }
        }
        Ok(ClientReply {
            status,
            headers,
            chunks,
            broke,
        })
    }

    /// Sends a request and returns as soon as the head of the answer has arrived.
    pub async fn open(&self, request: ClientRequest) -> Result<OpenReply, ClientError> {
        let response = self.builder(&request).send().await?;
        Ok(OpenReply {
            status: response.status(),
            headers: response.headers().clone(),
            response,
        })
    }
}
