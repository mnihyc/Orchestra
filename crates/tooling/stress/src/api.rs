//! The HTTP side: people who sign in, and requests timed and recorded by the route they are of.
//!
//! Every request names its route as the API's template (`/threads/{id}/sync`), so that the report groups the requests
//! of one route whatever their ids. Bodies are JSON values: the tool depends on no product crate, so a newer or an
//! older build of the API is driven alike, and a member it does not know is simply carried.

use crate::metrics::Recorder;
use anyhow::{Context, Result, bail};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, watch};

/// The answer to a request: its status (0 when none came) and its body (JSON, `null` when empty).
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Reply {
    /// Whether the status is a success.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The string at a JSON pointer of the body.
    pub fn text(&self, pointer: &str) -> Option<String> {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    /// The id of what was made: `id`, or `topic.id` where a reply wraps it.
    pub fn id(&self) -> Option<String> {
        self.text("/id").or_else(|| self.text("/topic/id"))
    }

    /// The status and the API's error code, for a message.
    pub fn why(&self) -> String {
        let code = self.text("/code").unwrap_or_default();
        let detail = self
            .text("/detail")
            .or_else(|| self.text("/title"))
            .unwrap_or_default();
        format!(
            "{} {} {}",
            self.status,
            code,
            detail.chars().take(240).collect::<String>()
        )
    }
}

/// A person signed in: their access token, renewed by signing in again shortly before it ends or when refused.
pub struct Person {
    pub login: String,
    pub id: String,
    password: String,
    token: watch::Sender<String>,
    renewing: AsyncMutex<()>,
}

impl Person {
    /// The access token of the moment.
    pub fn token(&self) -> String {
        self.token.borrow().clone()
    }

    /// Told of every new token (a socket sends `reauth` with it).
    pub fn tokens(&self) -> watch::Receiver<String> {
        self.token.subscribe()
    }
}

/// The API at one base address, with the recorder every request is written to.
pub struct Api {
    http: reqwest::Client,
    base: String,
    pub rec: Arc<Recorder>,
    /// The routes the server says it has (`GET /threads/{id}/sync`), once read from its OpenAPI document: a route an
    /// older or a newer build does not have is not asked, and the report says so once.
    known: OnceLock<HashSet<String>>,
    skipped: Mutex<BTreeSet<String>>,
}

/// Puts the arguments in place of the `{…}` of a route's template, in order.
fn fill(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len() + 40);
    let mut args = args.iter();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').map_or(rest.len(), |c| open + c + 1);
        out.push_str(args.next().copied().unwrap_or_default());
        rest = &rest[close..];
    }
    out.push_str(rest);
    out
}

/// A query string (`?a=1&b=two`) from pairs, encoded.
pub fn query(pairs: &[(&str, &str)]) -> String {
    if pairs.is_empty() {
        return String::new();
    }
    let mut s = form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        s.append_pair(k, v);
    }
    format!("?{}", s.finish())
}

impl Api {
    /// An API at `base` (`http://host:port/api/v1`).
    pub fn new(base: &str, rec: Arc<Recorder>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .tcp_nodelay(true)
            .pool_max_idle_per_host(1024)
            // Long enough for the slowest route seen (a list that asks every machine), short enough that a stuck
            // request is counted as one rather than holding a simulated person for ever.
            .timeout(Duration::from_secs(120))
            .build()
            .context("the HTTP client")?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            rec,
            known: OnceLock::new(),
            skipped: Mutex::new(BTreeSet::new()),
        })
    }

    /// Reads the server's own list of routes (`GET /openapi.json`), so that the drive asks only what this build has.
    pub async fn learn_routes(&self, who: &Person) {
        let reply = self.get(who, "/openapi.json", &[], "").await;
        let Some(paths) = reply.body.get("paths").and_then(Value::as_object) else {
            self.rec.note(format!(
                "the server's list of routes could not be read ({}): every route is asked",
                reply.why()
            ));
            return;
        };
        let mut known = HashSet::new();
        for (path, operations) in paths {
            for method in operations
                .as_object()
                .map(|o| o.keys().cloned().collect::<Vec<_>>())
                .unwrap_or_default()
            {
                if matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete") {
                    known.insert(format!("{} {path}", method.to_uppercase()));
                }
            }
        }
        let _ = self.known.set(known);
    }

    /// Whether the server said it has no such route.
    fn absent(&self, route: &str) -> bool {
        self.known.get().is_some_and(|known| !known.contains(route))
    }

    /// Signs a person in with their password.
    pub async fn sign_in(&self, login: &str, password: &str) -> Result<Arc<Person>> {
        let reply = self.login(login, password).await;
        if !reply.ok() {
            bail!("{login} could not sign in: {}", reply.why());
        }
        let token = reply
            .text("/access_token")
            .context("no access token in the sign-in reply")?;
        let id = reply
            .text("/user/id")
            .context("no user in the sign-in reply")?;
        let (token, _) = watch::channel(token);
        Ok(Arc::new(Person {
            login: login.to_owned(),
            id,
            password: password.to_owned(),
            token,
            renewing: AsyncMutex::new(()),
        }))
    }

    async fn login(&self, login: &str, password: &str) -> Reply {
        let body = json!({ "login": login, "password": password });
        self.send(
            None,
            Method::POST,
            "/auth/login",
            "/auth/login",
            Some(&body),
        )
        .await
    }

    /// Signs the person in again, unless another task already did since `stale` was their token.
    pub async fn renew(&self, who: &Person, stale: &str) -> bool {
        let _one_at_a_time = who.renewing.lock().await;
        if *who.token.borrow() != stale {
            return true;
        }
        let reply = self.login(&who.login, &who.password).await;
        match reply.text("/access_token") {
            Some(token) => {
                who.token.send_replace(token);
                true
            }
            None => false,
        }
    }

    /// One request as a person: `template` is the route (`/threads/{id}/inputs`), `args` fill its `{…}` in order,
    /// `query` is appended as it is. A refused token is renewed once and the request sent again.
    pub async fn call(
        &self,
        who: &Person,
        method: Method,
        template: &'static str,
        args: &[&str],
        query: &str,
        body: Option<&Value>,
    ) -> Reply {
        let path = format!("{}{}", fill(template, args), query);
        let token = who.token();
        let reply = self
            .send(Some(&token), method.clone(), template, &path, body)
            .await;
        if reply.status == 401 && self.renew(who, &token).await {
            let token = who.token();
            return self.send(Some(&token), method, template, &path, body).await;
        }
        reply
    }

    /// A GET as a person.
    pub async fn get(
        &self,
        who: &Person,
        template: &'static str,
        args: &[&str],
        query: &str,
    ) -> Reply {
        self.call(who, Method::GET, template, args, query, None)
            .await
    }

    /// A POST as a person.
    pub async fn post(
        &self,
        who: &Person,
        template: &'static str,
        args: &[&str],
        body: &Value,
    ) -> Reply {
        self.call(who, Method::POST, template, args, "", Some(body))
            .await
    }

    /// A POST of somebody who is not signed in (a person choosing their password through a link).
    pub async fn anonymous_post(&self, template: &'static str, body: &Value) -> Reply {
        self.send(None, Method::POST, template, template, Some(body))
            .await
    }

    /// A PUT as a person.
    pub async fn put(
        &self,
        who: &Person,
        template: &'static str,
        args: &[&str],
        body: &Value,
    ) -> Reply {
        self.call(who, Method::PUT, template, args, "", Some(body))
            .await
    }

    async fn send(
        &self,
        token: Option<&str>,
        method: Method,
        template: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Reply {
        let route = format!("{method} {template}");
        if self.absent(&route) {
            let first = self
                .skipped
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(route.clone());
            if first {
                self.rec
                    .note(format!("this build has no {route}: it is not asked"));
            }
            return Reply {
                status: 0,
                body: json!({ "code": "absent" }),
            };
        }
        let started = Instant::now();
        let mut request = self.http.request(method, format!("{}{}", self.base, path));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let answer = async {
            let response = request.send().await?;
            let status = response.status().as_u16();
            let bytes = response.bytes().await?;
            Ok::<_, reqwest::Error>((status, bytes))
        }
        .await;
        let took = started.elapsed().as_secs_f64() * 1000.0;
        match answer {
            Ok((status, bytes)) => {
                let body = if bytes.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                        Value::String(
                            String::from_utf8_lossy(&bytes[..bytes.len().min(400)]).into_owned(),
                        )
                    })
                };
                let code = body.get("code").and_then(Value::as_str);
                self.rec.request(&route, status, code, took);
                Reply { status, body }
            }
            Err(error) => {
                let kind = if error.is_timeout() {
                    "timeout"
                } else if error.is_connect() {
                    "connect"
                } else {
                    "transport"
                };
                self.rec.request(&route, 0, Some(kind), took);
                Reply {
                    status: 0,
                    body: json!({ "code": kind, "detail": error.to_string() }),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_filled_in_order() {
        assert_eq!(
            fill("/threads/{id}/items/{item_id}", &["a", "b"]),
            "/threads/a/items/b"
        );
        assert_eq!(fill("/meta", &[]), "/meta");
        assert_eq!(query(&[("q", "a b"), ("limit", "5")]), "?q=a+b&limit=5");
    }
}
