//! Transaction-local request authority using current identity, token scopes and the application clock.
//!
//! Call [`begin`] after decoding the body, before any action reads or writes.
//! The returned transaction holds credential and authorization locks through
//! commit. A declared permission is checked on that transaction automatically;
//! a rule handler uses the returned principal for its additional checks.
//!
//! There is no barrier every request shares. The check reads the
//! authorization epochs the write decides under (the system's, the person's root
//! orgs', the root org of the request's place, and of every node a decision reads)
//! in the statement that locks the identity, before any permission fact; the
//! commit checks them as its last statement and refuses, retryably
//! (`conflict.stale_revision`), when a change of authority committed meanwhile.
//! Roles, permission pages/defaults, memberships, groups, standing, moves and
//! shared availability/policy move those epochs at their own commit: the request's
//! root org for a change about one place, the system's otherwise. Ordinary content,
//! drafts, uploads and new children pin the existing object/parent. Personal
//! credentials lock their own user exclusively and watch no epoch. A revocation
//! cannot pass that person's action, while unrelated people may refresh, sign in
//! and work concurrently. The cache epoch is a separate row; creating an object
//! invalidates caches without moving any authorization epoch.
//!
//! Every changing request is named in [`LEDGER`], once, with where its authority is asked: in the
//! transaction that writes (`guarded`), not yet there (`open`, with why), or nowhere because it has no
//! signed-in caller (`separate`). The registry holds a request to what the ledger says of it: a changing
//! request named `guarded` that answers with success without having checked its guard is refused in a
//! build with debug assertions (every test that serves it proves the claim) and logged in any build.

use super::authentication::{self, Authed, Via};
use super::authorization::{self, refusals};
use super::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::routes::{Access, Route};
use orch_authz::Principal;
use orch_db::queries::transaction_guard::{self as rows, Credential};
use orch_db::{Conn, Tx};
use orch_types::Time;
use serde::Deserialize;
use std::cell::Cell;
use std::collections::HashMap;
use std::future::Future;
use std::sync::LazyLock;

tokio::task_local! {
    /// Whether the request being served has checked its action guard: set by every check that passes.
    static SEEN: Cell<bool>;
}

/// The ledger of every changing request: its name, where its authority is asked, and why.
pub const LEDGER: &str = include_str!("guard_ledger.json");

/// Where a changing request asks for its authority, as the ledger says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    /// In the transaction that writes, with the caller and the time of that check.
    Guarded,
    /// At its door only; the ledger says why it is not guarded yet.
    Open,
    /// Nowhere: it has no signed-in caller (signing in, registering, recovering).
    Separate,
}

/// One row of the ledger, as far as serving a request needs it.
#[derive(Deserialize)]
struct LedgerRow {
    name: String,
    boundary: Boundary,
}

/// The ledger read once. A ledger that does not read leaves every request to the strictest rule.
static BOUNDARIES: LazyLock<HashMap<String, Boundary>> = LazyLock::new(|| {
    serde_json::from_str::<Vec<LedgerRow>>(LEDGER)
        .unwrap_or_default()
        .into_iter()
        .map(|row| (row.name, row.boundary))
        .collect()
});

/// Where the ledger says `name` asks for its authority. A request it does not name takes the guard.
#[must_use]
pub fn boundary_of(name: &str) -> Boundary {
    BOUNDARIES.get(name).copied().unwrap_or(Boundary::Guarded)
}

/// Serves `work` and says whether it checked its action guard at least once while it ran.
pub(crate) async fn watch<T>(work: impl Future<Output = T>) -> (T, bool) {
    SEEN.scope(Cell::new(false), async move {
        let out = work.await;
        let seen = SEEN.try_with(|seen| seen.get()).unwrap_or(false);
        (out, seen)
    })
    .await
}

/// Holds a request that answered with success to what the ledger says of it: a changing request named
/// `guarded` that never checked its guard is logged, and refused in a build with debug assertions, so
/// that no test can pass through a write that did not ask for its authority again.
pub(crate) fn require_seen(route: &Route, seen: bool) -> ApiResult<()> {
    // A request served without a signed-in caller has no guard to check: it is separate by what it
    // is, as every such request of the ledger says, whether or not the ledger names it.
    let without_caller = matches!(route.access, Access::Public | Access::Cookie);
    if seen
        || route.method.as_str() == "GET"
        || without_caller
        || boundary_of(route.name) != Boundary::Guarded
    {
        return Ok(());
    }
    tracing::error!(
        request = route.name,
        "a changing request answered without checking its action guard"
    );
    if cfg!(debug_assertions) {
        return Err(ApiError::internal(format!(
            "{} answered without checking its action guard, which its ledger row says it does",
            route.name
        )));
    }
    Ok(())
}

/// Authority checked inside the transaction that performs the action.
#[derive(Debug)]
#[must_use = "rule checks use the guard's current principal, user and time"]
pub struct Guard {
    principal: Principal,
    user: Authed,
}

impl Guard {
    /// The principal whose current authority additional rule checks use.
    #[must_use]
    pub fn principal(&self) -> &Principal {
        &self.principal
    }

    /// The current user and credential, including administrator state and scopes.
    #[must_use]
    pub fn user(&self) -> &Authed {
        &self.user
    }

    /// When authority was checked, after any wait for a revocation transaction.
    #[must_use]
    pub fn now(&self) -> Time {
        self.principal.now()
    }

    pub(crate) fn into_principal(self) -> Principal {
        self.principal
    }
}

/// Begins the existing action transaction and checks current request authority.
pub async fn begin(ctx: &Ctx) -> ApiResult<(Tx, Guard)> {
    let mut tx = ctx.app.db.begin().await?;
    let guard = check(ctx, &mut tx).await?;
    Ok((tx, guard))
}

/// Checks current request authority in an already-open action transaction.
pub async fn check(ctx: &Ctx, tx: &mut Tx) -> ApiResult<Guard> {
    check_conn(ctx, tx).await
}

pub(crate) async fn check_conn(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Guard> {
    if !rows::in_transaction(conn) {
        return Err(ApiError::internal("an action guard needs a transaction"));
    }
    let original = ctx.user()?;
    let hash = authentication::bearer(&ctx.headers)
        .map(authentication::token_hash)
        .ok_or_else(invalid_token)?;
    let credential = match original.via {
        Via::Session { session_id } => Credential::Session {
            id: session_id,
            hash: &hash,
        },
        Via::Token { token_id, .. } => Credential::Token {
            id: token_id,
            hash: &hash,
        },
    };
    let authority_change = crate::registry::changes_authority(&ctx.route);
    let mut place = authorization::place_of(ctx);
    place.alone = authority_change && crate::registry::changes_one_place(&ctx.route);
    place.grants = authority_change && crate::registry::only_grants(&ctx.route);
    if authority_change && crate::registry::changes_standing(&ctx.route) {
        // Two changes of standing are ordered by their own row, before either reads anything.
        rows::hold_epoch(conn, rows::STANDING).await?;
        place.epoch = Some(rows::STANDING);
    } else if authority_change && !place.grants {
        // Two changes of the same scope are ordered before either reads anything: they wait
        // for each other, while ordinary writes check the scope's epoch at commit.
        rows::hold_change(conn, &place).await?;
    }
    let identity =
        rows::lock_identity_for_write(conn, original.id(), credential, authority_change, place)
            .await?
            .ok_or_else(invalid_token)?;
    let now = ctx.app.clock.now();
    if identity.expires_at.is_some_and(|end| end <= now) {
        return Err(invalid_token());
    }
    if identity.user.suspended() {
        return Err(ApiError::new(
            ErrorCode::AuthSuspended,
            "this user is suspended",
        ));
    }
    let via = match &original.via {
        Via::Session { session_id } => Via::Session {
            session_id: *session_id,
        },
        Via::Token { token_id, .. } => Via::Token {
            token_id: *token_id,
            scopes: identity
                .scopes
                .ok_or_else(|| ApiError::internal("an API token has no scopes"))?,
            node_id: identity.node_id,
        },
    };
    let user = Authed {
        as_admin: identity.as_admin,
        user: identity.user,
        via,
    };
    orch_db::audit::set_admin_mode(user.id(), user.as_admin && user.is_session());
    if matches!(ctx.route.access, Access::Session) && !user.is_session() {
        return Err(ApiError::new(
            ErrorCode::AuthSessionRequired,
            "this is done in a browser session, not with a token",
        ));
    }
    if ctx.route.path.starts_with("/admin/") && !user.is_system_admin() {
        return Err(refusals::system_admin_only(
            "only a system admin does this, in a browser session",
        ));
    }
    // Held out of the team orgs that wait for a second step of theirs, as a read is.
    let principal = authorization::principal_of(conn, &user, Time::new(now)).await?;
    if let Access::Permission(permission) = ctx.route.access {
        authorization::require_on_path_in(ctx, conn, &principal, permission).await?;
    }
    // Outside a request (a test that calls a handler's helpers directly) nothing is watching.
    let _ = SEEN.try_with(|seen| seen.set(true));
    Ok(Guard { principal, user })
}

fn invalid_token() -> ApiError {
    ApiError::new(
        ErrorCode::AuthInvalidToken,
        "the token is expired or unknown",
    )
}

pub(crate) fn changes(ctx: &Ctx) -> bool {
    ctx.route.method.as_str() != "GET"
}
