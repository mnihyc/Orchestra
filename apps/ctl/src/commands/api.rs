//! One request to the API with a token: the way without a browser.
//!
//! `orchctl api GET /me` sends the request to the system whose address is in
//! `ORCH_URL`, with the token in `ORCH_TOKEN`, and prints the answer as it
//! came back: the body, byte for byte, on standard output, whatever the
//! status. Anything the API offers can be asked this way, with the path as
//! the API's list of requests writes it (`/admin/users`); the base path
//! `/api/v1` is put in front of it.
//!
//! The status the program leaves says how it went: 0 for an answer in the
//! 200s, otherwise the first digit of the status the API answered (3, 4 or
//! 5), and 1 when no answer came at all or the request was wrong before it
//! was sent. One line on standard error says which when it was not a
//! success. A redirect is not followed, so that a token never goes on to a
//! place nobody named, and a token is never sent over plain `http` except to
//! the machine itself.

use super::console::{note, refuse};
use super::host;
use clap::Args;
use orch_api_contract::common::BASE_PATH;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use reqwest::{Method, Url};
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

/// The variable that holds the address of the system.
pub const URL_VARIABLE: &str = "ORCH_URL";

/// The variable that holds the API token the request is made with.
pub const TOKEN_VARIABLE: &str = "ORCH_TOKEN";

/// The methods the API's requests use.
const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

/// The most bytes of a body the API takes for any one request; a larger one
/// would be refused there, and is refused here with a better reason.
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// How long to wait for a connection: longer than a slow link needs.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// How long a request may take altogether, an answer that streams included:
/// five minutes, since an export of the audit trail streams for as long as it
/// takes, and a request that has not finished by then is not going to.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// The status left when no answer came, or the request was wrong before it
/// was sent: the status of every failure of this program.
const NO_ANSWER: u8 = 1;

/// One request to the API.
#[derive(Debug, Args)]
pub struct ApiArgs {
    /// The method: GET, POST, PUT, PATCH or DELETE.
    #[arg(value_name = "METHOD")]
    method: String,
    /// The path as the API lists it, such as `/me` or `/admin/users`, with a
    /// query string if the request has one: `/usage?scope=me`.
    #[arg(value_name = "PATH")]
    path: String,
    /// The body, as JSON: the text itself, `@file` to read it from a file,
    /// or `-` to read it from standard input.
    #[arg(long, value_name = "JSON")]
    data: Option<String>,
    /// A certificate authority to trust beside the ones the machine trusts,
    /// as a PEM file: for a system under an authority of its own, such as the
    /// one `compose-secrets` makes.
    #[arg(long, value_name = "FILE")]
    cacert: Option<PathBuf>,
}

/// Why no request was made, or no answer came.
#[derive(Debug, thiserror::Error)]
enum ApiError {
    #[error("ORCH_URL is not set: give the address of the system, as https://orch.example.com")]
    NoUrl,
    #[error("ORCH_URL is not an address: {0}")]
    BadUrl(String),
    #[error(
        "ORCH_URL is plain http, and a token must not cross a network unencrypted: use https:// (plain http is taken only for localhost)"
    )]
    PlainHttp,
    #[error("the path cannot be used: {0}")]
    BadPath(String),
    #[error(
        "ORCH_TOKEN is not set: give an API token, one that is made in the browser in the settings of your account"
    )]
    NoToken,
    #[error(
        "ORCH_TOKEN holds a character that cannot be sent in a header, a line break or a control character: is it the whole token and nothing else?"
    )]
    BadToken,
    #[error("{0:?} is not a method of the API: use GET, POST, PUT, PATCH or DELETE")]
    Method(String),
    #[error("a GET carries no body: put what it asks in the path, as /usage?scope=me")]
    GetWithBody,
    #[error("the path is empty: write the path of a request, such as /me")]
    NoPath,
    #[error("--data: cannot read {what}: {reason}")]
    ReadBody { what: String, reason: String },
    #[error("--data is not JSON: {0}")]
    NotJson(String),
    #[error("--data is larger than the 8 MiB the API takes for one request")]
    TooLarge,
    #[error("--cacert: {path}: {reason}")]
    CaCert { path: String, reason: String },
    #[error("the request to {url} failed: {reason}")]
    Failed { url: String, reason: String },
    #[error("cannot write the answer: {0}")]
    Output(io::Error),
}

/// The text of an error and of what caused it, as one line.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(next) = cause {
        text.push_str(": ");
        text.push_str(&next.to_string());
        cause = next.source();
    }
    text
}

/// A request that can be sent: nothing in it is left to be found wrong.
#[derive(Debug)]
struct Prepared {
    method: Method,
    url: Url,
    token: HeaderValue,
    body: Option<String>,
    authority: Vec<reqwest::Certificate>,
}

/// The method that was asked for.
fn method_of(given: &str) -> Result<Method, ApiError> {
    let upper = given.trim().to_ascii_uppercase();
    if !METHODS.contains(&upper.as_str()) {
        return Err(ApiError::Method(given.to_owned()));
    }
    Method::from_bytes(upper.as_bytes()).map_err(|_| ApiError::Method(given.to_owned()))
}

/// The address of the system, from `ORCH_URL`.
fn system_url(value: Option<String>) -> Result<Url, ApiError> {
    let value = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(ApiError::NoUrl)?;
    let url = Url::parse(&value).map_err(|error| ApiError::BadUrl(error.to_string()))?;
    match url.scheme() {
        "https" => {}
        "http" if is_this_machine(&url) => {}
        "http" => return Err(ApiError::PlainHttp),
        _ => {
            return Err(ApiError::BadUrl(
                "it begins with neither https:// nor http://".to_owned(),
            ));
        }
    }
    if url.host().is_none() {
        return Err(ApiError::BadUrl("it names no machine".to_owned()));
    }
    Ok(url)
}

/// Whether the address names the machine the program runs on.
fn is_this_machine(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// The address a path is requested at: the system's address, the base path of
/// the API unless the path already begins with `/api/`, and the path.
fn endpoint(base: &Url, path: &str) -> Result<Url, ApiError> {
    let path = path.trim();
    if path.is_empty() {
        return Err(ApiError::NoPath);
    }
    let rooted = if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    };
    let full = if rooted.starts_with("/api/") {
        rooted
    } else {
        format!("{BASE_PATH}{rooted}")
    };
    let prefix = base.as_str().trim_end_matches('/');
    Url::parse(&format!("{prefix}{full}")).map_err(|error| ApiError::BadPath(error.to_string()))
}

/// The body to send, from `--data`, which is JSON, or names where it is.
fn body_of(data: &str) -> Result<String, ApiError> {
    let (what, text) = if data == "-" {
        let mut text = String::new();
        let read = io::stdin()
            .lock()
            .take(MAX_BODY_BYTES + 1)
            .read_to_string(&mut text);
        ("standard input".to_owned(), read.map(|_| text))
    } else if let Some(path) = data.strip_prefix('@') {
        (path.to_owned(), std::fs::read_to_string(path))
    } else {
        (String::new(), Ok(data.to_owned()))
    };
    let text = text.map_err(|error| ApiError::ReadBody {
        what,
        reason: error.to_string(),
    })?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_BODY_BYTES {
        return Err(ApiError::TooLarge);
    }
    serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|error| ApiError::NotJson(error.to_string()))?;
    Ok(text)
}

/// The authorities to trust beside the machine's own.
fn authority_of(path: &Path) -> Result<Vec<reqwest::Certificate>, ApiError> {
    let bad = |reason: String| ApiError::CaCert {
        path: path.display().to_string(),
        reason,
    };
    let pem = std::fs::read(path).map_err(|error| bad(error.to_string()))?;
    reqwest::Certificate::from_pem_bundle(&pem).map_err(|error| bad(error.to_string()))
}

impl Prepared {
    /// Puts the request together from what was typed and what the
    /// environment holds.
    fn from(args: &ApiArgs, variable: impl Fn(&str) -> Option<String>) -> Result<Self, ApiError> {
        let method = method_of(&args.method)?;
        if method == Method::GET && args.data.is_some() {
            return Err(ApiError::GetWithBody);
        }
        let base = system_url(variable(URL_VARIABLE))?;
        let url = endpoint(&base, &args.path)?;
        let token = variable(TOKEN_VARIABLE)
            .map(|token| token.trim().to_owned())
            .filter(|token| !token.is_empty())
            .ok_or(ApiError::NoToken)?;
        let mut token =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| ApiError::BadToken)?;
        // Nothing that prints a request may print the token.
        token.set_sensitive(true);
        let body = args.data.as_deref().map(body_of).transpose()?;
        let authority = args.cacert.as_deref().map(authority_of).transpose()?;
        Ok(Self {
            method,
            url,
            token,
            body,
            authority: authority.unwrap_or_default(),
        })
    }
}

/// Sends the request and prints the answer; the status the API answered.
async fn send(request: Prepared) -> Result<u16, ApiError> {
    let failed = |error: &reqwest::Error| ApiError::Failed {
        url: request.url.to_string(),
        reason: chain(error),
    };
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("orchctl/", env!("ORCH_RELEASE_VERSION")));
    for certificate in &request.authority {
        builder = builder.add_root_certificate(certificate.clone());
    }
    let client = builder.build().map_err(|error| failed(&error))?;
    let mut call = client
        .request(request.method.clone(), request.url.clone())
        .header(AUTHORIZATION, request.token.clone());
    if let Some(body) = &request.body {
        call = call
            .header(CONTENT_TYPE, "application/json")
            .body(body.clone());
    }
    let mut answer = call.send().await.map_err(|error| failed(&error))?;
    let status = answer.status().as_u16();
    let mut out = io::stdout();
    let mut last = None;
    while let Some(chunk) = answer.chunk().await.map_err(|error| failed(&error))? {
        out.write_all(&chunk).map_err(ApiError::Output)?;
        last = chunk.last().copied().or(last);
    }
    // At a terminal the prompt must not be left on the answer's last line.
    if last.is_some_and(|byte| byte != b'\n') && out.is_terminal() {
        writeln!(out).map_err(ApiError::Output)?;
    }
    out.flush().map_err(ApiError::Output)?;
    if !answer.status().is_success() {
        note(format!(
            "{} {} answered {}",
            request.method,
            request.url.path(),
            answer.status()
        ));
    }
    Ok(status)
}

/// The status the program leaves for the status the API answered: 0 for the
/// 200s, the first digit of the status for the rest.
fn exit_status(answered: u16) -> u8 {
    match answered / 100 {
        2 => 0,
        class @ 3..=5 => u8::try_from(class).unwrap_or(NO_ANSWER),
        _ => NO_ANSWER,
    }
}

/// Makes the request and prints the answer.
pub(super) async fn json_request(
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
    cacert: Option<&Path>,
) -> Result<(u16, serde_json::Value), String> {
    let args = ApiArgs {
        method: method.to_owned(),
        path: path.to_owned(),
        data: body.map(|body| body.to_string()),
        cacert: cacert.map(Path::to_path_buf),
    };
    let request = Prepared::from(&args, |name| std::env::var(name).ok())
        .map_err(|error| error.to_string())?;
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent(concat!("orchctl/", env!("ORCH_RELEASE_VERSION")));
    for certificate in request.authority {
        builder = builder.add_root_certificate(certificate);
    }
    let client = builder.build().map_err(|error| error.to_string())?;
    let mut call = client
        .request(request.method, request.url)
        .header(AUTHORIZATION, request.token);
    if let Some(body) = request.body {
        call = call.header(CONTENT_TYPE, "application/json").body(body);
    }
    let answer = call
        .send()
        .await
        .map_err(|_| "the API could not be reached".to_owned())?;
    let status = answer.status().as_u16();
    let value = answer
        .json()
        .await
        .map_err(|_| "the API did not return a JSON sign-in state".to_owned())?;
    Ok((status, value))
}

/// Makes the request and prints the answer.
#[must_use]
pub fn run(args: &ApiArgs) -> ExitCode {
    let request = match Prepared::from(args, |name| std::env::var(name).ok()) {
        Ok(request) => request,
        Err(error) => return refuse(error),
    };
    match host::block_on(send(request)) {
        Ok(Ok(answered)) => ExitCode::from(exit_status(answered)),
        Ok(Err(error)) => refuse(error),
        Err(error) => refuse(error),
    }
}
