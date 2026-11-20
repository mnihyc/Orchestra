//! Overlapping browser openings can read the same raw bearer on one connection.
//! Each query serves only checks already waiting when that query starts.
use super::{Authed, Caller, Via, bearer, invalid_token, on_surface, token_hash};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::state::AppState;
use orch_api_contract::{common::ErrorCode, surface::HttpSurface};
use orch_db::{
    Db,
    queries::identity::{self, BrowserSession},
};
use orch_types::Time;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
    time::Instant,
};
use tokio::sync::Mutex as AsyncMutex;

const ROUTE: &str = "ThreadOpeningGet";

struct Sample {
    before_query: Instant,
    row: Option<Arc<BrowserSession>>,
}

#[derive(Default)]
struct Pending {
    waiters: usize,
    sample: Arc<AsyncMutex<Option<Sample>>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    application: usize,
    token_hash: Vec<u8>,
}

#[derive(Default)]
pub(crate) struct Reads(Mutex<HashMap<Key, Pending>>);

struct Check<'a> {
    owner: &'a Reads,
    key: Key,
    sample: Arc<AsyncMutex<Option<Sample>>>,
    arrived: Instant,
}

impl Reads {
    fn enroll(&self, key: Key) -> Check<'_> {
        let mut lanes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let pending = lanes.entry(key.clone()).or_default();
        pending.waiters += 1;
        Check {
            owner: self,
            key,
            sample: pending.sample.clone(),
            arrived: Instant::now(),
        }
    }
}

impl Drop for Check<'_> {
    fn drop(&mut self) {
        let mut lanes = self.owner.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(lane) = lanes.get_mut(&self.key) {
            lane.waiters -= 1;
            if lane.waiters == 0 {
                lanes.remove(&self.key);
            }
        }
    }
}

impl Check<'_> {
    async fn read(&self, db: &Db) -> ApiResult<Option<Arc<BrowserSession>>> {
        crate::testing::hold_boundary(ROUTE, "opening.identity_enrolled").await;
        let mut sample = self.sample.lock().await;
        // Cut before sending the statement. An arrival during SQL (or at an
        // indistinguishable clock tick) must do another read for its own check.
        if let Some(current) = sample.as_ref()
            && self.arrived < current.before_query
        {
            crate::testing::note_boundary(ROUTE, "opening.identity_joined");
            return Ok(current.row.clone());
        }
        let (before_query, row) = {
            let mut conn = db.acquire().await?;
            let before_query = Instant::now();
            let row = identity::browser_session(&mut conn, &self.key.token_hash)
                .await?
                .map(Arc::new);
            (before_query, row)
        };
        crate::testing::note_boundary(ROUTE, "opening.identity_sampled");
        crate::testing::hold_boundary(ROUTE, "opening.identity_sampled").await;
        *sample = Some(Sample {
            before_query,
            row: row.clone(),
        });
        Ok(row)
    }
}

async fn current(app: &AppState, token: &str) -> ApiResult<Arc<BrowserSession>> {
    let check = app.cache.opening_sessions().enroll(Key {
        application: std::ptr::from_ref(app) as usize,
        token_hash: token_hash(token),
    });
    let Some(row) = check
        .read(&app.db)
        .await?
        .filter(|row| app.clock.now() < row.credential_expires_at)
    else {
        let mut conn = app.db.acquire().await?;
        return Err(
            super::browser_refusal(&mut conn, &check.key.token_hash, app.clock.now()).await?,
        );
    };
    if row.user.suspended() {
        return Err(
            ApiError::new(ErrorCode::AuthSuspended, "this user is suspended")
                .session_ended(super::unavailable_session()),
        );
    }
    Ok(row)
}

pub(crate) async fn at_entry(
    app: &AppState,
    headers: &http::HeaderMap,
) -> ApiResult<(Caller, Option<Time>)> {
    on_surface(HttpSurface::Browser, headers)?;
    let Some(token) = bearer(headers) else {
        return Ok((Caller::Anonymous, None));
    };
    let row = current(app, token).await?;
    Ok((
        Caller::User(Arc::new(Authed {
            via: Via::Session {
                session_id: row.session_id,
            },
            user: row.user.clone(),
            as_admin: row.as_admin,
        })),
        Some(Time::new(row.credential_expires_at)),
    ))
}

pub(crate) async fn check_delivery(ctx: &Ctx) -> ApiResult<()> {
    let row = current(&ctx.app, bearer(&ctx.headers).ok_or_else(invalid_token)?).await?;
    for user in [ctx.authenticated_user()?, ctx.user()?] {
        if user.id() != row.user.id
            || !matches!(user.via, Via::Session { session_id } if session_id == row.session_id)
        {
            return Err(invalid_token());
        }
    }
    ctx.publication
        .retain_deadline(Some(Time::new(row.credential_expires_at)))
}
