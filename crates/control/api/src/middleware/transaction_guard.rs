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
use super::authorization;
use super::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::state::App;
use futures::future::BoxFuture;
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
pub(crate) fn watch<'a, T: 'a>(
    work: impl Future<Output = T> + Send + 'a,
) -> BoxFuture<'a, (T, bool)> {
    let work: BoxFuture<'a, T> = Box::pin(work);
    Box::pin(async move {
        SEEN.scope(Cell::new(false), async move {
            let out = work.await;
            let seen = SEEN.try_with(|seen| seen.get()).unwrap_or(false);
            (out, seen)
        })
        .await
    })
}

pub(crate) fn reset_snapshot_read() {
    let _ = SEEN.try_with(|seen| seen.set(false));
}

pub(crate) fn checked_snapshot_read() {
    let _ = SEEN.try_with(|seen| seen.set(true));
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
    // Current original HTTP row lifetime, retained through any later native preparation.
    expires_at: Option<Time>,
    operation_data: crate::key_operations::resolver::OperationData,
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

    /// Operands captured by this exact guarded preparation. They cannot be replaced by
    /// an older request-preparation snapshot or by another parallel read of the context.
    pub fn operation_data<T: Send + Sync + 'static>(&self) -> ApiResult<std::sync::Arc<T>> {
        self.operation_data.get()
    }

    /// Recheck this actual raw HTTP/key lifetime immediately before a first effect,
    /// after any final effect-free native preparation. This refreshes no authority,
    /// adds no permission and must not be applied to accepted reconciliation/cleanup.
    pub(crate) fn check_deadline(&self, ctx: &Ctx) -> ApiResult<()> {
        deadline(ctx, self.expires_at, self.principal.caller(), None)
    }

    /// Carry the exact transport error through the native writer without changing its meaning.
    pub(crate) fn before_effect(&self, ctx: &Ctx) -> Result<(), orch_engine::EngineError> {
        self.check_deadline(ctx)
            .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
    }

    pub(crate) fn into_principal(self) -> Principal {
        self.principal
    }
}

/// Capture only this real request's raw credential association for later native checkpoints.
/// No native Session or caller admission is constructed from an account id.
pub(crate) async fn native_origin(
    ctx: &Ctx,
) -> ApiResult<(
    orch_engine::operation_scope::request_origin::Credential,
    Option<Time>,
)> {
    use orch_engine::operation_scope::request_origin::Credential;
    let mut tx = ctx.app.db.begin().await?;
    let identity = current_http_identity(ctx, &mut tx, HttpIdentityMode::Read).await?;
    let hash = authentication::bearer(&ctx.headers)
        .map(authentication::token_hash)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the original HTTP credential has ended",
            )
        })?;
    let credential = match &identity.user.via {
        Via::Session { session_id } => Credential::Session {
            id: *session_id,
            hash,
        },
        Via::Token { token_id, .. } => Credential::Token {
            id: *token_id,
            hash,
        },
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    identity.check_deadline(&ctx.app)?;
    Ok((credential, identity.expires_at))
}

/// Begins the existing action transaction and checks current request authority.
pub async fn begin(ctx: &Ctx) -> ApiResult<(Tx, Guard)> {
    let mut tx = if ctx.snapshot_read() {
        ctx.app.db.begin_snapshot().await?
    } else {
        ctx.app.db.begin().await?
    };
    let guard = check(ctx, &mut tx).await?;
    Ok((tx, guard))
}

/// Checks current request authority in an already-open action transaction.
pub async fn check(ctx: &Ctx, tx: &mut Tx) -> ApiResult<Guard> {
    check_conn(ctx, tx).await
}

pub(crate) async fn check_conn(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Guard> {
    if ctx.snapshot_read() {
        return check_snapshot_read(ctx, conn).await;
    }
    if ctx.topic_creation_prefix().is_some() {
        return Err(ApiError::internal(
            "compound topic creation needs its private transaction stage",
        ));
    }
    check_conn_mode(ctx, conn, None).await
}

/// A declared effect-free read has one coherent snapshot and one native preparation.
/// Mutation discovery remains separate: reusing its pre-wait protected operands is
/// not safe. Reads instead defer entry preparation and resolve here exactly once.
pub(crate) async fn check_snapshot_read(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Guard> {
    let frame = crate::key_operations::native::current_read_frame(ctx, conn).await?;
    Ok(Guard {
        operation_data: frame.ctx.operation_data_snapshot()?,
        principal: frame.principal,
        user: frame.identity.user,
        expires_at: frame.identity.expires_at,
    })
}

pub(crate) async fn check_topic_prefix(
    ctx: &Ctx,
    conn: &mut Conn,
    access: &crate::key_operations::creation_stage::PrefixAccess,
) -> ApiResult<Guard> {
    if ctx.topic_creation_prefix().is_none() {
        return Err(ApiError::internal(
            "a creation prefix guard needs its exact request marker",
        ));
    }
    check_conn_mode(ctx, conn, Some(access)).await
}

/// Existing identity lock modes; Read checks only the original HTTP identity and
/// does not acquire operation authority or re-admit a completed mutation.
#[derive(Clone, Copy)]
pub(crate) enum HttpIdentityMode {
    Action,
    Read,
}

/// Current authentication and original/current ceilings, never serialized as authority.
pub(crate) struct CurrentHttpIdentity {
    pub(crate) user: Authed,
    pub(crate) caller: orch_authz::Caller,
    pub(crate) now: Time,
    pub(crate) expires_at: Option<Time>,
}

impl CurrentHttpIdentity {
    /// The same raw-credential and conjunctive key deadline formula used by the action Guard.
    pub(crate) fn check_deadline(&self, app: &App) -> ApiResult<()> {
        deadline_for(app.clock.as_ref(), self.expires_at, &self.caller, None)
    }
}

/// Read facts for a real transport origin. The id/hash must come from its actual authenticator;
/// a native Session caller or another credential of the same account cannot substitute for it.
pub(crate) async fn current_read_identity(
    app: &App,
    conn: &mut Conn,
    user: orch_types::Id,
    credential: Credential<'_>,
) -> ApiResult<CurrentHttpIdentity> {
    let identity = rows::lock_identity(conn, user, credential, false)
        .await?
        .ok_or_else(invalid_token)?;
    identity_facts(app, credential, identity)
}

/// Hydrate current authentication from the exact locked credential row. Caller ceilings and
/// operation admission remain with each transport's existing checkpoint.
fn identity_facts(
    app: &App,
    credential: Credential<'_>,
    identity: rows::Identity,
) -> ApiResult<CurrentHttpIdentity> {
    let expires_at = identity.expires_at.map(Time::new);
    let via = match credential {
        Credential::Session { id, .. } => Via::Session { session_id: id },
        Credential::Token { id, .. } => Via::Token {
            token_id: id,
            scopes: identity
                .scopes
                .ok_or_else(|| ApiError::internal("an API token has no scopes"))?,
            node_id: identity.node_id,
            operation_scope: authentication::operation_scope(identity.operation_scope)?,
            expires_at,
        },
    };
    let user = Authed {
        as_admin: identity.as_admin && matches!(credential, Credential::Session { .. }),
        user: identity.user,
        via,
    };
    let caller = authorization::caller_of(&user)?;
    let facts = CurrentHttpIdentity {
        user,
        caller,
        now: app.clock.time(),
        expires_at,
    };
    facts.check_deadline(app)?;
    Ok(facts)
}

/// The one exact HTTP credential boundary for actions and optional responses.
/// Session identity/hash is checked here; a native human Session origin is not a substitute.
pub(crate) async fn current_http_identity(
    ctx: &Ctx,
    conn: &mut Conn,
    mode: HttpIdentityMode,
) -> ApiResult<CurrentHttpIdentity> {
    if matches!(mode, HttpIdentityMode::Action) && !rows::in_transaction(conn) {
        return Err(ApiError::internal("an action guard needs a transaction"));
    }
    let original = ctx.authenticated_user()?;
    let request_user = ctx.user()?;
    let same = request_user.id() == original.id()
        && match (&request_user.via, &original.via) {
            (Via::Session { session_id: a }, Via::Session { session_id: b }) => a == b,
            (Via::Token { token_id: a, .. }, Via::Token { token_id: b, .. }) => a == b,
            _ => false,
        };
    if !same {
        return Err(ApiError::internal(
            "HTTP authentication changed credential origin",
        ));
    }
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
    let identity = match mode {
        HttpIdentityMode::Action => {
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
            rows::lock_identity_for_write(conn, original.id(), credential, authority_change, place)
                .await?
        }
        HttpIdentityMode::Read => {
            rows::lock_identity(conn, original.id(), credential, false).await?
        }
    }
    .ok_or_else(invalid_token)?;
    let current = identity_facts(&ctx.app, credential, identity)?;
    let expires_at = current.expires_at;
    let mut user = current.user;
    let mut caller = authorization::caller_of(original)?;
    for accepted in ctx
        .operation_ceiling()
        .into_iter()
        .chain(ctx.admitted_caller())
    {
        if accepted.user() != original.id()
            || match (&original.via, accepted.grant()) {
                (Via::Session { .. }, None) => false,
                (Via::Token { token_id, .. }, Some(grant)) => {
                    !grant.token_ids().any(|id| id == *token_id) || accepted.as_admin()
                }
                _ => true,
            }
        {
            return Err(ApiError::internal(
                "accepted caller changed its HTTP credential origin",
            ));
        }
        caller = caller
            .intersect_restrictions(accepted)
            .map_err(ApiError::internal)?;
    }
    caller = caller
        .intersect_restrictions(&authorization::caller_of(&user)?)
        .map_err(ApiError::internal)?;
    // Additional accepted key ceilings remain conjunctive. The lower primitive
    // checks their current rows and live minimum deadline after any identity wait.
    if caller.grant().is_some() {
        caller = orch_authz::CapturedCaller::capture(&caller)
            .restore_current(conn, ctx.app.clock.as_ref())
            .await
            .map_err(authorization::restoration_error)?
            .ok_or_else(invalid_token)?;
    }
    let now = ctx.app.clock.time();
    deadline(ctx, expires_at, &caller, None)?;
    // The explicit mode applies to every native administrator level. The
    // ordinary Principal checks current org/department/system standing.
    // A changed system flag cannot lend a previously prepared admin context.
    let effective_admin = caller.as_admin()
        && original.is_session()
        && user.is_session()
        && original.user.is_system_admin == user.user.is_system_admin;
    caller = caller.with_admin_mode(effective_admin);
    user.as_admin = caller.as_admin();
    ctx.publication.retain_deadline(expires_at)?;
    ctx.publication
        .retain_deadline(caller.grant().and_then(orch_authz::TokenGrant::expires_at))?;
    Ok(CurrentHttpIdentity {
        user,
        caller,
        now,
        expires_at,
    })
}

/// Refresh after the producer's complete native row/capacity serialization footprint,
/// before its first unaccepted effect. This never checks a completed action/cleanup.
pub(crate) async fn after_wait(ctx: &Ctx, conn: &mut Conn) -> ApiResult<(Ctx, Guard)> {
    let guard = check_conn(ctx, conn).await?;
    let mut current = ctx.for_operation_checkpoint(guard.user().clone(), guard.now().get());
    // Legacy unclassified callers are not operation proofs. Their original HTTP
    // ceiling is retained separately, while typed operations install actual admission.
    if guard.principal().caller().admitted_operation().is_some() {
        current = current.with_admitted_caller(guard.principal().caller().clone())?;
    }
    Ok((current, guard))
}

fn check_conn_mode<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    topic_prefix: Option<&'a crate::key_operations::creation_stage::PrefixAccess>,
) -> BoxFuture<'a, ApiResult<Guard>> {
    Box::pin(async move {
        // Discovery is metadata-only: retain the ordinary early refusal/lock order,
        // but no pre-wait Principal, protected operands or disclosure is final.
        let discovered = current_http_identity(ctx, conn, HttpIdentityMode::Action).await?;
        let discovery = ctx.for_effect_discovery(discovered.user.clone(), discovered.now.get());
        let provisional = prepare_guard(&discovery, conn, discovered, topic_prefix).await;
        if let Err(error) = provisional {
            // A failed database snapshot cannot be reused or refreshed in place. Keep
            // the existing typed retry unchanged and expose no provisional rich payload.
            if error.is_database_retry() {
                return Err(authorization::problem_without_content(error));
            }
            // Effect-free ordinary/scope/validation errors are provisional too. Rebuild
            // their actual current decision below rather than lend a pre-wait error body.
        }
        // Identity locks are reentrant; targets taken by discovery remain held. The
        // actual producer and ordinary gates run again with fresh account/ACL time.
        // A producer acquiring additional late locks still needs its explicit after_wait
        // footprint; two passes are not a general claim that arbitrary awaits are safe.
        let current = current_http_identity(ctx, conn, HttpIdentityMode::Action).await?;
        let expires_at = current.expires_at;
        let accepted_deadline = current.caller.clone();
        let checkpoint = ctx.for_operation_checkpoint(current.user.clone(), current.now.get());
        let prepared = prepare_guard(&checkpoint, conn, current, topic_prefix).await;
        // Hold errors as well as successes until this final synchronous lifetime
        // boundary: an expired request cannot publish an old rich native refusal.
        // This changes no still-valid native decision and retimes no protected data.
        deadline(
            ctx,
            expires_at,
            &accepted_deadline,
            prepared
                .as_ref()
                .ok()
                .map(|guard| guard.principal().caller()),
        )?;
        let guard = prepared?;
        ctx.publish_operation_data(guard.operation_data.clone())?;
        let _ = SEEN.try_with(|seen| seen.set(true));
        Ok(guard)
    })
}

/// Rebuild the actual plan, protected operands, Principal and existing ordinary gates
/// together. No native action, trial commit, RPC or durable effect is replayed here.
fn prepare_guard<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    identity: CurrentHttpIdentity,
    topic_prefix: Option<&'a crate::key_operations::creation_stage::PrefixAccess>,
) -> BoxFuture<'a, ApiResult<Guard>> {
    Box::pin(async move {
        let CurrentHttpIdentity {
            user,
            caller,
            now,
            expires_at,
        } = identity;
        guard_user(&user)?;
        // Rebuild scope facts on this guarded connection. The request's earlier proof
        // supplies only accepted ceilings, never authoritative paths after a move.
        let mut principal = authorization::load_principal(conn, caller, now).await?;
        principal = crate::sign_in::second_step::hold(conn, &user, principal).await?;
        let scoped_key = principal
            .caller()
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some())
            || ctx
                .admitted_caller()
                .and_then(|caller| caller.grant())
                .is_some_and(|grant| grant.operation_scope().is_some());
        let mut operation_data = crate::key_operations::resolver::OperationData::default();
        if let Some(access) = topic_prefix {
            let caller = crate::key_operations::creation_stage::current_prefix_caller(
                ctx,
                conn,
                principal.caller(),
                access,
            )
            .await?;
            principal = authorization::load_principal(conn, caller, now).await?;
            principal = crate::sign_in::second_step::hold(conn, &user, principal).await?;
            crate::key_operations::creation_stage::require_prefix_parent(
                ctx, conn, &principal, access,
            )
            .await?;
        } else if scoped_key || ctx.operation_request().is_some() {
            let caller = match ctx.admitted_caller() {
                Some(accepted) if accepted.grant().is_some() => accepted
                    .intersect_restrictions(principal.caller())
                    .map_err(ApiError::internal)?,
                _ => principal.caller().clone(),
            };
            let current = ctx.for_operation_checkpoint(user.clone(), now.get());
            let admission =
                crate::key_operations::resolver::resolve(&current, conn, &caller).await?;
            let admitted = match admission {
                Some(admission) => {
                    let admission = admission.complete()?;
                    ctx.checkpoint_disclosure(admission.disclosure);
                    operation_data = admission.data;
                    authorization::load_principal(conn, admission.caller, now).await?
                }
                None => {
                    crate::key_operations::resolver::ordinary_unclassified(&caller)?;
                    authorization::load_principal(conn, caller, now).await?
                }
            };
            principal = crate::sign_in::second_step::hold(conn, &user, admitted).await?;
        }
        finish_guard(
            ctx,
            conn,
            Guard {
                principal,
                user,
                expires_at,
                operation_data,
            },
            scoped_key,
        )
        .await
    })
}

pub(crate) fn guard_user(user: &Authed) -> ApiResult<()> {
    if user.user.suspended() {
        return Err(ApiError::new(
            ErrorCode::AuthSuspended,
            "this user is suspended",
        ));
    }
    orch_db::audit::set_admin_mode(user.id(), user.as_admin && user.is_session());
    Ok(())
}

/// Complete the shared native adapter's read preparation without resolving it again.
pub(crate) async fn finish_read_guard(
    ctx: &Ctx,
    conn: &mut Conn,
    identity: &CurrentHttpIdentity,
    principal: Principal,
) -> ApiResult<Guard> {
    let scoped_key = identity
        .caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some());
    finish_guard(
        ctx,
        conn,
        Guard {
            principal,
            user: identity.user.clone(),
            expires_at: identity.expires_at,
            operation_data: ctx.operation_data_snapshot()?,
        },
        scoped_key,
    )
    .await
}

/// Both paths retain the same native route-level session and permission gates.
async fn finish_guard(
    ctx: &Ctx,
    conn: &mut Conn,
    guard: Guard,
    scoped_key: bool,
) -> ApiResult<Guard> {
    if matches!(ctx.route.access, Access::Session) && !guard.user.is_session() && !scoped_key {
        return Err(ApiError::new(
            ErrorCode::AuthSessionRequired,
            "this is done in a browser session, not with a token",
        ));
    }
    if let Access::Permission(permission) = ctx.route.access {
        authorization::require_on_path_in(ctx, conn, &guard.principal, permission).await?;
    }
    Ok(guard)
}

/// Complete the same locked prefix guard with full operation admission. The private creation
/// stage calls this before exposing its transaction to any dependent creation producer.
pub(crate) async fn complete_topic_prefix(
    ctx: &Ctx,
    conn: &mut Conn,
    guard: Guard,
    admission: crate::key_operations::admission::Admission,
    _access: &crate::key_operations::creation_stage::PrefixAccess,
) -> ApiResult<(Ctx, Guard)> {
    if ctx.topic_creation_prefix().is_none() {
        return Err(ApiError::internal(
            "a complete creation guard needs its prefix",
        ));
    }
    let operation_data = admission.data.clone();
    let complete = crate::key_operations::admission::install(ctx.clone(), admission)?;
    // The complete proof belongs to this already-locked synchronous transaction. Calling
    // `principal(ctx)` would re-enter the ordinary guard and its deliberate prefix refusal.
    let admitted = complete
        .admitted_caller()
        .ok_or_else(|| ApiError::internal("the complete creation lost its admitted caller"))?;
    let principal = authorization::admitted_principal(
        conn,
        admitted,
        &guard.user,
        Time::new(ctx.app.clock.now()),
    )
    .await?;
    if let Access::Permission(permission) = ctx.route.access {
        authorization::require_on_path_in(ctx, conn, &principal, permission).await?;
    }
    let guard = Guard {
        principal,
        user: guard.user,
        expires_at: guard.expires_at,
        operation_data,
    };
    guard.check_deadline(ctx)?;
    Ok((complete, guard))
}

// One existing lifetime formula for the raw HTTP row and every retained/current
// key ceiling. Current identity/provisional errors remain outside this pure check.
fn deadline(
    ctx: &Ctx,
    expires_at: Option<Time>,
    accepted: &orch_authz::Caller,
    current: Option<&orch_authz::Caller>,
) -> ApiResult<()> {
    deadline_for(ctx.app.clock.as_ref(), expires_at, accepted, current)
}

fn deadline_for(
    clock: &dyn orch_types::Clock,
    expires_at: Option<Time>,
    accepted: &orch_authz::Caller,
    current: Option<&orch_authz::Caller>,
) -> ApiResult<()> {
    if expires_at.is_some_and(|end| end <= clock.time())
        || !accepted.key_deadline_allows(clock)
        || current.is_some_and(|caller| !caller.key_deadline_allows(clock))
    {
        return Err(invalid_token());
    }
    Ok(())
}

fn invalid_token() -> ApiError {
    ApiError::new(
        ErrorCode::AuthInvalidToken,
        "the token is expired or unknown",
    )
}

pub(crate) fn changes(ctx: &Ctx) -> bool {
    ctx.route.method.as_str() != "GET" && !ctx.snapshot_read()
}

#[cfg(test)]
mod deadline_tests;
