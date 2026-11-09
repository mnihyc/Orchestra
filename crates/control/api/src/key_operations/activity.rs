//! Metadata for one authenticated key call, shared by HTTP and realtime producers.
//!
//! This collector accepts resolved operation facts only. Request payloads, raw URLs,
//! headers and error messages deliberately have no place in its stored record.

pub(crate) use super::activity_store::record;

use orch_access_contract::key_scope::KeyFacet;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::routes::Route;
use orch_types::{Id, Time};
use std::future::Future;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The authenticated origin; an unknown or expired credential never creates one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KeyOrigin {
    pub key_id: Id,
    pub account_id: Id,
}

/// Facts retained for the bounded recent-call history. Target names are projected
/// later under the reader's current authority, never copied into this record.
#[derive(Debug, Clone)]
pub(crate) struct FinishedCall {
    pub origin: KeyOrigin,
    pub at: Time,
    pub operation: String,
    pub selection: Option<String>,
    pub facets: Vec<KeyFacet>,
    pub method: String,
    pub route: String,
    pub status: u16,
    pub error: Option<ErrorCode>,
    pub replayed: bool,
    pub duration_ms: u64,
    pub request_id: String,
    pub remote: Option<IpAddr>,
}

#[derive(Debug, Default)]
struct State {
    origin: Option<KeyOrigin>,
    selection: Option<String>,
    facets: Vec<KeyFacet>,
    replayed: bool,
    finished: bool,
}

/// A clone belongs to the same logical call, including a transaction retry. A
/// real idempotency replay is marked by its producer, never inferred from retries.
#[derive(Debug, Clone)]
pub(crate) struct Call {
    state: Arc<Mutex<State>>,
    at: Time,
    started: Instant,
    operation: String,
    method: String,
    route: String,
    request_id: String,
    remote: Option<IpAddr>,
}

tokio::task_local! {
    static CURRENT: Call;
}

impl Call {
    pub fn http(route: Route, request_id: String, at: Time, remote: Option<IpAddr>) -> Self {
        Self {
            state: Arc::default(),
            at,
            started: Instant::now(),
            operation: route.name.to_owned(),
            method: route.method.as_str().to_owned(),
            route: route.path.to_owned(),
            request_id,
            remote,
        }
    }

    pub async fn scope<T>(&self, work: impl Future<Output = T>) -> T {
        CURRENT.scope(self.clone(), work).await
    }

    /// Called only after the credential and account have been authenticated.
    pub fn authenticated(&self, origin: KeyOrigin) {
        if let Ok(mut state) = self.state.lock()
            && state.origin.is_none()
        {
            state.origin = Some(origin);
        }
    }

    /// The shared operation producer supplies its canonical variant and resolved
    /// facets before admission, so authenticated scope refusals remain observable.
    pub fn selected(&self, selection: &str, facets: &[KeyFacet]) {
        if let Ok(mut state) = self.state.lock() {
            state.selection = Some(selection.to_owned());
            state.facets = facets.to_vec();
        }
    }

    pub fn replayed(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.replayed = true;
        }
    }

    /// Exactly one record per logical call; a cloned handle cannot double-log it.
    pub fn finish(&self, status: u16, error: Option<ErrorCode>) -> Option<FinishedCall> {
        let mut state = self.state.lock().ok()?;
        let origin = state.origin?;
        if state.finished {
            return None;
        }
        state.finished = true;
        Some(FinishedCall {
            origin,
            at: self.at,
            operation: self.operation.clone(),
            selection: state.selection.clone(),
            facets: state.facets.clone(),
            method: self.method.clone(),
            route: self.route.clone(),
            status,
            error,
            replayed: state.replayed,
            duration_ms: self
                .started
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
            request_id: self.request_id.clone(),
            remote: self.remote,
        })
    }
}

/// Does nothing outside an observed HTTP/realtime call, such as a direct unit test.
pub(crate) fn authenticated(origin: KeyOrigin) {
    let _ = CURRENT.try_with(|call| call.authenticated(origin));
}

pub(crate) fn selected(selection: &str, facets: &[KeyFacet]) {
    let _ = CURRENT.try_with(|call| call.selected(selection, facets));
}

pub(crate) fn replayed() {
    let _ = CURRENT.try_with(Call::replayed);
}
