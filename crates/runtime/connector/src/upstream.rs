//! The HTTP exchange with an upstream model service: a connection made for the request, the request sent whole, and the answer handed on piece by piece as it arrives.
//!
//! A model call is long and its answer is a stream, so the exchange is shaped for that: one connection for
//! each request (plain TCP for an `http` endpoint that policy allowed, TLS for an `https` one, verified
//! against the roots the system trusts and the public ones a connector brings), the request body sent whole
//! because the forwarder has read it already, and the answer's head waited for as long as a model may think
//! before it says a word. The answer's body is not read here: [`Relay`] is the body the harness is given, and
//! it pulls a piece from the upstream only when the harness's connection can take one, so a slow reader slows
//! the upstream and nothing is held in between. A [`Watcher`] sees every piece pass, which is how usage is
//! read without delaying anything, and is told how the body ended: whole, cut by the upstream, stalled, or
//! given up on by the harness.

use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::{Body, Frame, Incoming, SizeHint};
use hyper::header::{HOST, HeaderMap, HeaderValue};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use orch_pki::PkiError;
use orch_pki::verification::public_client_config;
use orch_types::Clock;
use rustls::RootCertStore;
use rustls::pki_types::ServerName;
use tokio::time::Sleep;
use tokio_rustls::TlsConnector;
use url::Url;

use crate::https::CONNECT_TIMEOUT;

/// A streaming filter for credential values echoed by an upstream.
#[derive(Clone)]
pub(crate) struct CredentialMask {
    expression: regex::bytes::Regex,
    longest: usize,
    replacement: &'static [u8],
}

impl CredentialMask {
    /// Protects the values sent as credentials and their JSON-escaped forms.
    pub(crate) fn new(values: &[String]) -> Result<Option<Self>, regex::Error> {
        let mut patterns = Vec::new();
        for value in values.iter().filter(|value| !value.is_empty()) {
            patterns.push(value.clone());
            if let Ok(quoted) = serde_json::to_string(value) {
                patterns.push(quoted[1..quoted.len() - 1].to_owned());
            }
        }
        patterns.sort_unstable_by_key(|value| std::cmp::Reverse(value.len()));
        patterns.dedup();
        let Some(longest) = patterns.first().map(String::len) else {
            return Ok(None);
        };
        let expression = patterns
            .iter()
            .map(|value| regex::escape(value))
            .collect::<Vec<_>>()
            .join("|");
        let expression = regex::bytes::Regex::new(&expression)?;
        let marker = orch_plugin_contract::secret::REDACTED.as_bytes();
        let replacement: &'static [u8] = if expression.is_match(marker) {
            b""
        } else {
            marker
        };
        Ok(Some(Self {
            expression,
            longest,
            replacement,
        }))
    }

    /// A response header with any echoed credential replaced.
    pub(crate) fn header(&self, value: &HeaderValue) -> Option<HeaderValue> {
        HeaderValue::from_bytes(
            &self
                .expression
                .replace_all(value.as_bytes(), self.replacement),
        )
        .ok()
    }

    /// Masks an entire UTF-8 message.
    pub(crate) fn text(&self, text: &str) -> String {
        let safe = self
            .expression
            .replace_all(text.as_bytes(), self.replacement);
        String::from_utf8(safe.into_owned()).unwrap_or_default()
    }

    /// Masks a whole reply read for model discovery.
    pub(crate) fn bytes(&self, bytes: &[u8]) -> Bytes {
        Bytes::from(
            self.expression
                .replace_all(bytes, self.replacement)
                .into_owned(),
        )
    }
}

/// The uncommitted end of a stream, which may contain the start of a credential.
struct MaskBuffer {
    mask: CredentialMask,
    pending: Vec<u8>,
}

impl MaskBuffer {
    fn new(mask: CredentialMask) -> Self {
        Self {
            mask,
            pending: Vec::new(),
        }
    }

    fn take(&mut self, last: bool) -> Bytes {
        let limit = if last {
            self.pending.len()
        } else {
            self.pending
                .len()
                .saturating_sub(self.mask.longest.saturating_sub(1))
        };
        let mut output = Vec::new();
        let mut consumed = 0;
        for found in self.mask.expression.find_iter(&self.pending) {
            if found.start() >= limit {
                break;
            }
            output.extend_from_slice(&self.pending[consumed..found.start()]);
            output.extend_from_slice(self.mask.replacement);
            consumed = found.end();
        }
        let boundary = limit.max(consumed);
        output.extend_from_slice(&self.pending[consumed..boundary]);
        self.pending.drain(..boundary);
        Bytes::from(output)
    }
}

/// Masks an upstream body without exposing a credential split across two frames.
pub(crate) struct MaskedBody<B> {
    inner: B,
    buffer: MaskBuffer,
    ended: bool,
}

impl<B> MaskedBody<B> {
    pub(crate) fn new(inner: B, mask: CredentialMask) -> Self {
        Self {
            inner,
            buffer: MaskBuffer::new(mask),
            ended: false,
        }
    }
}

impl<B> Body for MaskedBody<B>
where
    B: Body<Data = Bytes, Error = std::io::Error> + Unpin,
{
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        if self.ended {
            return Poll::Ready(None);
        }
        loop {
            match Pin::new(&mut self.inner).poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => {
                    if let Ok(piece) = frame.into_data() {
                        self.buffer.pending.extend_from_slice(&piece);
                        let safe = self.buffer.take(false);
                        if !safe.is_empty() {
                            return Poll::Ready(Some(Ok(Frame::data(safe))));
                        }
                    }
                }
                Poll::Ready(Some(Err(error))) => {
                    self.buffer.pending.clear();
                    self.ended = true;
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    self.ended = true;
                    let safe = self.buffer.take(true);
                    return if safe.is_empty() {
                        Poll::Ready(None)
                    } else {
                        Poll::Ready(Some(Ok(Frame::data(safe))))
                    };
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.ended
    }
}

/// How long the head of an answer may take. A model that is asked for a long answer in one document, and
/// not as a stream, says nothing until it has finished, and the longest such request a vendor documents is
/// ten minutes; a request that takes longer is meant to be streamed.
pub const HEAD_TIMEOUT: Duration = Duration::from_secs(600);
/// How long a stream may be silent between two pieces. A stream carries events, or the keep-alive comments
/// a vendor sends, much more often than this; five minutes of nothing is an upstream that has hung.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// The most of an answer that is read whole (a list of models): the list of a gateway that serves every
/// model of the world is a few megabytes.
pub const MAX_WHOLE_REPLY_BYTES: usize = 16 * 1024 * 1024;

/// Why an upstream could not be asked.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpstreamError {
    /// The address is not one that can be reached.
    #[error("not an address that can be reached: {0}")]
    Address(String),
    /// The connection could not be made.
    #[error("the upstream could not be reached: {0}")]
    Connect(String),
    /// The TLS handshake failed: the upstream is not who its address says.
    #[error("the upstream's certificate was not accepted: {0}")]
    Tls(String),
    /// The exchange failed.
    #[error("the exchange with the upstream failed: {0}")]
    Http(String),
    /// The upstream took longer than it may.
    #[error("the upstream took too long to {0}")]
    TimedOut(&'static str),
}

/// A request to an upstream.
#[derive(Debug)]
pub struct UpstreamRequest {
    /// The method.
    pub method: Method,
    /// The whole address, the query included.
    pub url: Url,
    /// The headers, the credential among them; `Host` is set here.
    pub headers: HeaderMap,
    /// The body.
    pub body: Bytes,
}

/// The head of an upstream's answer, and its body not yet read.
#[derive(Debug)]
pub struct UpstreamResponse {
    /// The status.
    pub status: StatusCode,
    /// The headers.
    pub headers: HeaderMap,
    /// The body.
    pub body: Incoming,
}

/// The roots an upstream's certificate is checked against: those of the operating system, and the public
/// ones a connector carries, so that an upstream with a public certificate is reached on a machine whose own
/// store is bare, and a model gateway of a company, whose authority the machine's owner installed, on one
/// that has it.
pub use orch_pki::verification::trusted_roots;

/// Makes requests to upstreams.
#[derive(Clone)]
pub struct Upstream {
    tls: Arc<rustls::ClientConfig>,
}

impl std::fmt::Debug for Upstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upstream").finish_non_exhaustive()
    }
}

fn host_header(url: &Url) -> Option<HeaderValue> {
    let host = url.host_str()?;
    let text = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    };
    HeaderValue::from_str(&text).ok()
}

impl Upstream {
    /// A client that checks certificates against `roots`; the time of the check is the clock's.
    pub fn with_roots(roots: Arc<RootCertStore>, clock: Arc<dyn Clock>) -> Result<Self, PkiError> {
        Ok(Self {
            tls: public_client_config(roots, clock)?,
        })
    }

    /// A client that trusts the roots of [`trusted_roots`].
    pub fn new(clock: Arc<dyn Clock>) -> Result<Self, PkiError> {
        Self::with_roots(trusted_roots(), clock)
    }

    /// Sends the request and waits for the head of the answer.
    pub async fn send(&self, request: UpstreamRequest) -> Result<UpstreamResponse, UpstreamError> {
        let url = &request.url;
        let host = url
            .host_str()
            .ok_or_else(|| UpstreamError::Address(url.to_string()))?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_owned();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| UpstreamError::Address(url.to_string()))?;
        let secure = match url.scheme() {
            "https" => true,
            "http" => false,
            other => {
                return Err(UpstreamError::Address(format!(
                    "{other}: only http and https upstreams are reached"
                )));
            }
        };
        // Through the system's proxy, where it names one (a restricted runtime's is its host's).
        let proxy = orch_link::outward::OutwardProxy::system();
        let tcp = tokio::time::timeout(
            CONNECT_TIMEOUT,
            orch_link::outward::connect(host.as_str(), port, secure, Some(&proxy)),
        )
        .await
        .map_err(|_| UpstreamError::TimedOut("connect"))?
        .map_err(|error| UpstreamError::Connect(error.to_string()))?;
        tcp.set_nodelay(true)
            .map_err(|error| UpstreamError::Connect(error.to_string()))?;
        if secure {
            let name = ServerName::try_from(host.clone())
                .map_err(|_| UpstreamError::Address(format!("`{host}` is not a name to verify")))?;
            let stream = TlsConnector::from(Arc::clone(&self.tls))
                .connect(name, tcp)
                .await
                .map_err(|error| UpstreamError::Tls(error.to_string()))?;
            exchange(TokioIo::new(stream), request).await
        } else {
            exchange(TokioIo::new(tcp), request).await
        }
    }

    /// Sends the request and reads the whole answer, up to `limit` bytes of body.
    pub async fn fetch(
        &self,
        request: UpstreamRequest,
        limit: usize,
    ) -> Result<(StatusCode, HeaderMap, Bytes), UpstreamError> {
        let answer = self.send(request).await?;
        let body = tokio::time::timeout(HEAD_TIMEOUT, Limited::new(answer.body, limit).collect())
            .await
            .map_err(|_| UpstreamError::TimedOut("answer"))?
            .map_err(|error| UpstreamError::Http(error.to_string()))?
            .to_bytes();
        Ok((answer.status, answer.headers, body))
    }
}

async fn exchange<I>(io: I, request: UpstreamRequest) -> Result<UpstreamResponse, UpstreamError>
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(io)
        .await
        .map_err(|error| UpstreamError::Http(error.to_string()))?;
    // The connection lives as long as the answer is being read, then ends by itself.
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut path_and_query = request.url.path().to_owned();
    if let Some(query) = request.url.query() {
        path_and_query.push('?');
        path_and_query.push_str(query);
    }
    let mut outgoing = Request::builder()
        .method(request.method)
        .uri(path_and_query)
        .body(Full::new(request.body))
        .map_err(|error| UpstreamError::Http(error.to_string()))?;
    *outgoing.headers_mut() = request.headers;
    if let Some(host) = host_header(&request.url) {
        outgoing.headers_mut().insert(HOST, host);
    }
    let answer = tokio::time::timeout(HEAD_TIMEOUT, sender.send_request(outgoing))
        .await
        .map_err(|_| UpstreamError::TimedOut("answer"))?
        .map_err(|error| UpstreamError::Http(error.to_string()))?;
    let (parts, body) = answer.into_parts();
    Ok(UpstreamResponse {
        status: parts.status,
        headers: parts.headers,
        body,
    })
}

/// How a body ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// To its end.
    Whole,
    /// The upstream broke it off, or stopped sending: the reason.
    Cut(String),
    /// Whoever was reading it stopped before its end.
    Abandoned,
}

/// Something that watches a body pass.
pub trait Watcher: Send {
    /// A piece of the body passed by.
    fn piece(&mut self, piece: &[u8]);

    /// The body ended, and how. Called once.
    fn end(&mut self, end: End);
}

/// The body of an answer, handed on as it arrives, watched on its way.
pub struct Relay {
    inner: Pin<Box<Incoming>>,
    watcher: Mutex<Option<Box<dyn Watcher>>>,
    idle: Duration,
    timer: Pin<Box<Sleep>>,
    done: bool,
}

impl std::fmt::Debug for Relay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Relay").finish_non_exhaustive()
    }
}

impl Relay {
    /// The body of an answer, watched by `watcher`, given up on after `idle` without a piece.
    #[must_use]
    pub fn new(body: Incoming, watcher: Box<dyn Watcher>, idle: Duration) -> Self {
        Self {
            inner: Box::pin(body),
            watcher: Mutex::new(Some(watcher)),
            idle,
            timer: Box::pin(tokio::time::sleep(idle)),
            done: false,
        }
    }

    fn finish(&mut self, end: End) {
        self.done = true;
        let watcher = self
            .watcher
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut watcher) = watcher {
            watcher.end(end);
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        if !self.done {
            self.finish(End::Abandoned);
        }
    }
}

impl Body for Relay {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = &mut *self;
        if this.done {
            return Poll::Ready(None);
        }
        match this.inner.as_mut().poll_frame(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(piece) = frame.data_ref()
                    && let Some(watcher) = this
                        .watcher
                        .get_mut()
                        .unwrap_or_else(PoisonError::into_inner)
                        .as_mut()
                {
                    watcher.piece(piece);
                }
                let next = tokio::time::Instant::now() + this.idle;
                this.timer.as_mut().reset(next);
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.finish(End::Cut(error.to_string()));
                Poll::Ready(Some(Err(std::io::Error::other(error))))
            }
            Poll::Ready(None) => {
                this.finish(End::Whole);
                Poll::Ready(None)
            }
            Poll::Pending => {
                if this.timer.as_mut().poll(cx).is_ready() {
                    this.finish(End::Cut("the upstream stopped sending".to_owned()));
                    return Poll::Ready(Some(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "the upstream stopped sending",
                    ))));
                }
                Poll::Pending
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done || self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    #[test]
    fn credentials_are_masked_across_frames_including_overlapping_and_json_escaped_values() {
        let mask = CredentialMask::new(&["token".into(), "token-long".into(), "quote\"key".into()])
            .unwrap()
            .unwrap();
        let mut buffer = MaskBuffer::new(mask.clone());
        let mut seen = Vec::new();
        for piece in [
            b"prefix tok".as_slice(),
            br#"en-long then quote\""#.as_slice(),
            b"key suffix".as_slice(),
        ] {
            buffer.pending.extend_from_slice(piece);
            seen.extend_from_slice(&buffer.take(false));
        }
        seen.extend_from_slice(&buffer.take(true));
        assert_eq!(
            String::from_utf8(seen).unwrap(),
            "prefix [secret] then [secret] suffix"
        );
        let header = HeaderValue::from_static("token-long rejected");
        assert_eq!(mask.header(&header).unwrap(), "[secret] rejected");
        let marker_is_secret = CredentialMask::new(&["secret".into()]).unwrap().unwrap();
        assert_eq!(
            marker_is_secret
                .header(&HeaderValue::from_static("secret rejected"))
                .unwrap(),
            " rejected"
        );
    }

    #[derive(Default)]
    struct Seen {
        pieces: Mutex<Vec<u8>>,
        ended: Mutex<Option<End>>,
    }

    struct Recording(Arc<Seen>);

    impl Watcher for Recording {
        fn piece(&mut self, piece: &[u8]) {
            self.0.pieces.lock().unwrap().extend_from_slice(piece);
        }
        fn end(&mut self, end: End) {
            *self.0.ended.lock().unwrap() = Some(end);
        }
    }

    fn clock() -> Arc<dyn Clock> {
        Arc::new(orch_types::SystemClock)
    }

    async fn serve_once(script: Vec<(Duration, &'static [u8])>) -> (Url, Arc<AtomicBool>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = Url::parse(&format!("http://{}/answer", listener.local_addr().unwrap())).unwrap();
        let closed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&closed);
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut seen = vec![0u8; 4096];
            let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut seen).await;
            for (pause, bytes) in script {
                tokio::time::sleep(pause).await;
                if socket.write_all(bytes).await.is_err() {
                    flag.store(true, Ordering::SeqCst);
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = socket.shutdown().await;
        });
        (url, closed)
    }

    fn request(url: Url) -> UpstreamRequest {
        UpstreamRequest {
            method: Method::GET,
            url,
            headers: HeaderMap::new(),
            body: Bytes::new(),
        }
    }

    #[tokio::test]
    async fn the_first_piece_of_an_answer_is_handed_on_before_the_upstream_has_finished() {
        let (url, _) = serve_once(vec![
            (
                Duration::ZERO,
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n6\r\nfirst \r\n",
            ),
            (Duration::from_millis(1500), b"6\r\nsecond\r\n0\r\n\r\n"),
        ])
        .await;
        let upstream = Upstream::new(clock()).unwrap();
        let answer = upstream.send(request(url)).await.unwrap();
        let seen = Arc::new(Seen::default());
        let mut relay = Relay::new(
            answer.body,
            Box::new(Recording(Arc::clone(&seen))),
            IDLE_TIMEOUT,
        );
        let started = tokio::time::Instant::now();
        let first = std::future::poll_fn(|cx| Pin::new(&mut relay).poll_frame(cx))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.into_data().unwrap(), "first ");
        assert!(
            started.elapsed() < Duration::from_millis(1000),
            "the first piece waited for the rest: {:?}",
            started.elapsed()
        );
        assert!(seen.ended.lock().unwrap().is_none());
        while std::future::poll_fn(|cx| Pin::new(&mut relay).poll_frame(cx))
            .await
            .is_some()
        {}
        assert_eq!(*seen.ended.lock().unwrap(), Some(End::Whole));
        assert_eq!(&*seen.pieces.lock().unwrap(), b"first second");
    }

    #[tokio::test]
    async fn an_upstream_that_breaks_the_stream_off_is_seen_as_cut_and_the_reader_gets_an_error() {
        let (url, _) = serve_once(vec![(
            Duration::ZERO,
            b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n6\r\nfirst \r\n9\r\nsecon",
        )])
        .await;
        let answer = Upstream::new(clock())
            .unwrap()
            .send(request(url))
            .await
            .unwrap();
        let seen = Arc::new(Seen::default());
        let mut relay = Relay::new(
            answer.body,
            Box::new(Recording(Arc::clone(&seen))),
            IDLE_TIMEOUT,
        );
        let mut failed = false;
        while let Some(next) = std::future::poll_fn(|cx| Pin::new(&mut relay).poll_frame(cx)).await
        {
            if next.is_err() {
                failed = true;
            }
        }
        assert!(failed, "the reader must see the break");
        assert!(matches!(
            seen.ended.lock().unwrap().clone(),
            Some(End::Cut(_))
        ));
    }

    #[tokio::test]
    async fn a_stream_that_goes_silent_is_given_up_after_the_idle_time() {
        let (url, _) = serve_once(vec![
            (
                Duration::ZERO,
                b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n6\r\nfirst \r\n",
            ),
            (Duration::from_secs(3600), b"0\r\n\r\n"),
        ])
        .await;
        let answer = Upstream::new(clock())
            .unwrap()
            .send(request(url))
            .await
            .unwrap();
        // The clock stops only once the connection stands: a stopped clock moves on whenever nothing is ready, so
        // it can run out the connect deadline while a real connection is still being made (seen on macOS).
        tokio::time::pause();
        let seen = Arc::new(Seen::default());
        let mut relay = Relay::new(
            answer.body,
            Box::new(Recording(Arc::clone(&seen))),
            Duration::from_secs(5),
        );
        let mut results = Vec::new();
        while let Some(next) = std::future::poll_fn(|cx| Pin::new(&mut relay).poll_frame(cx)).await
        {
            results.push(next.is_ok());
        }
        assert_eq!(results, [true, false]);
        assert!(matches!(
            seen.ended.lock().unwrap().clone(),
            Some(End::Cut(_))
        ));
    }

    #[tokio::test]
    async fn a_reader_that_stops_before_the_end_is_told_to_the_watcher() {
        let (url, _) = serve_once(vec![
            (
                Duration::ZERO,
                b"HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n6\r\nfirst \r\n",
            ),
            (Duration::from_millis(300), b"0\r\n\r\n"),
        ])
        .await;
        let answer = Upstream::new(clock())
            .unwrap()
            .send(request(url))
            .await
            .unwrap();
        let seen = Arc::new(Seen::default());
        let mut relay = Relay::new(
            answer.body,
            Box::new(Recording(Arc::clone(&seen))),
            IDLE_TIMEOUT,
        );
        let _ = std::future::poll_fn(|cx| Pin::new(&mut relay).poll_frame(cx)).await;
        drop(relay);
        assert_eq!(*seen.ended.lock().unwrap(), Some(End::Abandoned));
    }

    #[tokio::test]
    async fn an_address_that_is_neither_http_nor_https_is_not_reached() {
        let upstream = Upstream::new(clock()).unwrap();
        let outcome = upstream
            .send(request(Url::parse("ftp://example.com/x").unwrap()))
            .await;
        assert!(matches!(outcome, Err(UpstreamError::Address(_))));
    }
}
