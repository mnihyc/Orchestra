//! Live port measurements and single-use channel admissions over the existing backplane.

use crate::middleware::authorization::RetainedIdentity;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::ports::*;
use orch_db::queries::environments::routes::EnvironmentFull;
use orch_link::tunnels::channel::Tunnel;
use orch_types::{Id, Time};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// Ordinary single-use entry life, allowing a few slow-link round trips.
pub const USE_SECONDS: i64 = 30;
/// The normal current live-query deadline of the port command.
const QUERY_DEADLINE: Duration = Duration::from_secs(15);
struct Use {
    ctx: Ctx,
    environment: Id,
    generation: i64,
    port: u16,
    transport: PortTransport,
    opening: Option<Id>,
    /// Made by a link's holder: the link is asked again, there is no signed-in caller.
    by_link: bool,
    expires: Time,
    proof: String,
}
/// Whom a live channel was admitted for, which says how its admission is asked again.
#[derive(Clone)]
enum Who {
    /// A signed-in caller, asked again with their own credential (a helper, own use).
    Session(Box<Ctx>),
    /// A link's holder: the link is asked again.
    Link,
    /// A person who entered by the browser route, asked again by who they are.
    Person(RetainedIdentity),
}

#[derive(Clone)]
struct Active {
    app: crate::state::App,
    who: Who,
    environment: Id,
    generation: i64,
    port: u16,
    transport: PortTransport,
    opening: Option<Id>,
    closer: orch_link::tunnels::channel::TunnelCloser,
}

/// A browser's entry to one port's stable name: a person by their sign-in, or a link's holder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserEntry {
    /// The person who entered, or none for a link's holder.
    pub person: Option<Id>,
    /// The authenticated session that admitted this person, absent for link holders.
    pub(crate) identity: Option<RetainedIdentity>,
    /// The environment of the port.
    pub environment: Id,
    /// The environment incarnation that admitted the entry.
    pub(crate) generation: i32,
    /// The port.
    pub port: u16,
    /// The named-place or link opening it entered by, or none for own use.
    pub opening: Option<Id>,
    /// Until when it holds without entering again.
    pub expires: Time,
}

/// How long a browser's entry holds before it enters again: a working day. Every request it
/// makes asks its admission again; this only bounds how long the cookie is remembered.
pub const BROWSER_ENTRY_HOURS: i64 = 12;
/// The most browser entries one API process remembers, so that entering many times cannot grow
/// its memory without end; past it the oldest go first.
const BROWSER_ENTRIES_MAX: usize = 4_096;
/// The bytes of an entry code and of an entry cookie.
const ENTRY_TOKEN_BYTES: usize = 32;

fn entry_token() -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(rand::random::<[u8; ENTRY_TOKEN_BYTES]>())
}

/// Only current measurements and short-lived admission handles are held here.
#[derive(Default)]
pub struct PortRuntime {
    snapshots: Mutex<HashMap<Id, PortSnapshot>>,
    uses: Mutex<HashMap<Id, Use>>,
    active: Mutex<HashMap<Id, Active>>,
    watch_queries: Mutex<HashMap<Id, (Id, Id, i64)>>,
    codes: Mutex<HashMap<String, BrowserEntry>>,
    browsers: Mutex<HashMap<String, BrowserEntry>>,
}
impl std::fmt::Debug for PortRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortRuntime").finish_non_exhaustive()
    }
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PortRuntime {
    /// Accepts only a fresh background query's admitted machine/environment/generation report.
    pub fn hear(&self, report: &crate::backplane::Report) {
        if report.eph.kind != "ports.snapshot" {
            return;
        }
        let Ok(orch_link_contract::events::Report::PortsSnapshot(reply)) = report.eph.report()
        else {
            return;
        };
        let Some(reference) = reply.request_ref else {
            return;
        };
        let expected = lock(&self.watch_queries).remove(&reference);
        if expected
            == Some((
                report.machine_id,
                reply.snapshot.environment_id,
                reply.snapshot.generation,
            ))
        {
            self.observe(reply.snapshot);
        }
    }
    /// The last live scoped measurement, with its original time and generation.
    pub fn snapshot(&self, environment: Id, generation: i64) -> Option<PortSnapshot> {
        lock(&self.snapshots)
            .get(&environment)
            .filter(|s| s.generation == generation)
            .cloned()
    }
    /// Keeps only the latest live measurement; nothing is journalled or reconstructed.
    pub fn observe(&self, snapshot: PortSnapshot) {
        let mut kept = lock(&self.snapshots);
        if kept.get(&snapshot.environment_id).is_none_or(|old| {
            snapshot.generation > old.generation
                || (snapshot.generation == old.generation && snapshot.as_of >= old.as_of)
        }) {
            kept.insert(snapshot.environment_id, snapshot);
        }
    }
    /// Reads the actual connector report on a fresh reference, never a retained command result.
    pub async fn look(&self, ctx: &Ctx, environment: &EnvironmentFull) -> ApiResult<PortSnapshot> {
        let machine = environment.machine_id.ok_or_else(|| {
            ApiError::new(
                ErrorCode::PortsUnavailable,
                "the environment has no current connector",
            )
        })?;
        if environment.status != "running" || !ctx.app.link.is_online(machine) {
            return Err(ApiError::new(
                ErrorCode::PortsUnavailable,
                "the environment is stopped or offline",
            ));
        }
        let request_ref = orch_types::new_id();
        let mut reports = ctx.app.link.reports();
        let body = orch_link_contract::ports::PortsLook {
            environment_id: environment.id,
            generation: i64::from(environment.generation),
            request_ref,
        };
        ctx.app
            .link
            .dispatcher()
            .issue(
                machine,
                &orch_link_contract::commands::SerialKey::Environment(environment.id),
                &body,
            )
            .await
            .map_err(|_| {
                ApiError::new(ErrorCode::PortsUnavailable, "the connector cannot be asked")
            })?;
        tokio::time::timeout(QUERY_DEADLINE, async {
            loop {
                let report = reports.recv().await.map_err(|_| {
                    ApiError::new(ErrorCode::PortsUnavailable, "the live report was lost")
                })?;
                if report.machine_id != machine || report.eph.kind != "ports.snapshot" {
                    continue;
                }
                let Ok(orch_link_contract::events::Report::PortsSnapshot(reply)) =
                    report.eph.report()
                else {
                    continue;
                };
                if reply.request_ref == Some(request_ref)
                    && reply.snapshot.environment_id == environment.id
                    && reply.snapshot.generation == i64::from(environment.generation)
                {
                    self.observe(reply.snapshot.clone());
                    return Ok(reply.snapshot);
                }
            }
        })
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::PortsUnavailable,
                "the current live report did not arrive",
            )
        })?
    }
    /// Keeps one ephemeral admission of a signed-in caller with a bounded total of unused handles.
    #[expect(
        clippy::too_many_arguments,
        reason = "the canonical use fields and current request are independently fenced"
    )]
    pub fn admit(
        &self,
        ctx: &Ctx,
        environment: &EnvironmentFull,
        port: u16,
        transport: PortTransport,
        opening: Option<Id>,
        ticket: String,
        expires: Time,
        id: Id,
    ) -> ApiResult<PortUse> {
        self.admit_as(
            ctx,
            environment,
            port,
            transport,
            opening,
            false,
            ticket,
            expires,
            id,
        )
    }

    /// Keeps one ephemeral admission of a link's holder, by the link's opening.
    #[expect(
        clippy::too_many_arguments,
        reason = "the canonical use fields and current request are independently fenced"
    )]
    pub fn admit_link(
        &self,
        ctx: &Ctx,
        environment: &EnvironmentFull,
        port: u16,
        transport: PortTransport,
        link: Id,
        ticket: String,
        expires: Time,
        id: Id,
    ) -> ApiResult<PortUse> {
        self.admit_as(
            ctx,
            environment,
            port,
            transport,
            Some(link),
            true,
            ticket,
            expires,
            id,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the canonical use fields and current request are independently fenced"
    )]
    fn admit_as(
        &self,
        ctx: &Ctx,
        environment: &EnvironmentFull,
        port: u16,
        transport: PortTransport,
        opening: Option<Id>,
        by_link: bool,
        ticket: String,
        expires: Time,
        id: Id,
    ) -> ApiResult<PortUse> {
        let now = Time::new(ctx.app.clock.now());
        let mut uses = lock(&self.uses);
        uses.retain(|_, usage| usage.expires > now);
        // No more unused handles than the environment could carry connections and senders.
        let limits = orch_link::ports::limits_of(&ctx.app.config.link);
        let unused_handles = orch_link::ports::count(limits.connections_max)
            .saturating_add(orch_link::ports::count(limits.waiting_max))
            .saturating_add(orch_link::ports::count(limits.udp_senders_max));
        if uses
            .values()
            .filter(|u| u.environment == environment.id)
            .count()
            >= unused_handles
        {
            return Err(
                ApiError::new(ErrorCode::PortsTooMany, "unused port admissions are full")
                    .retry_after(USE_SECONDS as u64),
            );
        }
        uses.insert(
            id,
            Use {
                ctx: ctx.clone(),
                environment: environment.id,
                generation: i64::from(environment.generation),
                port,
                transport,
                opening,
                by_link,
                expires,
                proof: ticket.clone(),
            },
        );
        Ok(PortUse {
            id,
            environment_id: environment.id,
            port,
            transport,
            opening_id: opening,
            ticket,
            ticket_expires_at: expires,
        })
    }
    /// Opens a channel once after the entry owner observes its actual outside peer.
    /// The peer comes from the transport, never a client-selected address.
    pub async fn open_channel(
        &self,
        id: Id,
        proof: &str,
        outside: SocketAddr,
    ) -> ApiResult<Tunnel> {
        let usage = {
            let mut uses = lock(&self.uses);
            if uses.get(&id).is_none_or(|u| u.proof != proof) {
                return Err(ApiError::new(
                    ErrorCode::PortsNotAllowed,
                    "the use is unknown or consumed",
                ));
            }
            uses.remove(&id).ok_or_else(ApiError::not_found)?
        };
        if usage.expires <= Time::new(usage.ctx.app.clock.now()) {
            return Err(ApiError::new(
                ErrorCode::PortsNotAllowed,
                "the port use expired",
            ));
        }
        let now = Time::new(usage.ctx.app.clock.now());
        let (tx, env, caller, revision, audience) = if usage.by_link {
            let mut tx = usage.ctx.app.db.begin().await?;
            let env = crate::routes::ports::environment(&mut tx, usage.environment).await?;
            if i64::from(env.generation) != usage.generation {
                return Err(ApiError::new(
                    ErrorCode::PortsUnavailable,
                    "the environment generation changed",
                ));
            }
            let row = orch_db::queries::ports::opening(
                &mut tx,
                usage.opening.ok_or_else(ApiError::not_found)?,
            )
            .await?
            .ok_or_else(ApiError::not_found)?;
            let caller =
                crate::routes::ports::may_use_link(&mut tx, &env, &row, usage.transport, now)
                    .await?;
            (
                tx,
                env,
                caller,
                row.revision,
                Some(orch_engine::port_capabilities::kind_of(&row.kind)),
            )
        } else {
            let (mut tx, guard) = crate::middleware::transaction_guard::begin(&usage.ctx).await?;
            let env = crate::routes::ports::environment(&mut tx, usage.environment).await?;
            if i64::from(env.generation) != usage.generation {
                return Err(ApiError::new(
                    ErrorCode::PortsUnavailable,
                    "the environment generation changed",
                ));
            }
            crate::routes::ports::may_use(
                &mut tx,
                guard.principal(),
                &env,
                usage.port,
                usage.transport,
                usage.opening,
            )
            .await?;
            let user = crate::middleware::authorization::refusals::user_refs(
                &mut tx,
                &[guard.principal().user()],
            )
            .await?
            .remove(&guard.principal().user())
            .ok_or_else(ApiError::not_found)?;
            let (revision, audience) = if let Some(opening) = usage.opening {
                let row = orch_db::queries::ports::opening(&mut tx, opening)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                (
                    row.revision,
                    Some(orch_engine::port_capabilities::kind_of(&row.kind)),
                )
            } else {
                (env.revision, None)
            };
            (tx, env, PortCaller::Person { user }, revision, audience)
        };
        let claims = orch_link_contract::ports::PortTicketClaims {
            environment_id: env.id,
            generation: usage.generation,
            port: usage.port,
            transport: usage.transport,
            opening_id: usage.opening,
            audience,
            authority_revision: revision,
            allocation_revision: None,
            host_template: None,
            source: PortConnectionOutside {
                address: outside.ip().to_string(),
                port: outside.port(),
                caller,
            },
        };
        let machine = env
            .machine_id
            .ok_or_else(|| ApiError::new(ErrorCode::PortsUnavailable, "the connector is absent"))?;
        let ticket = usage
            .ctx
            .app
            .link
            .tickets()
            .port_use(id, machine, claims, usage.expires)
            .map_err(ApiError::internal)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let tunnel = usage
            .ctx
            .app
            .link
            .terminals()
            .open(machine, &ticket)
            .await
            .map_err(|_| {
                ApiError::new(
                    ErrorCode::PortsNotListening,
                    "the current owned target could not open",
                )
            })?;
        let active = Active {
            app: usage.ctx.app.clone(),
            who: if usage.by_link {
                Who::Link
            } else {
                Who::Session(Box::new(usage.ctx))
            },
            environment: usage.environment,
            generation: usage.generation,
            port: usage.port,
            transport: usage.transport,
            opening: usage.opening,
            closer: tunnel.closer(),
        };
        if Self::current(&active).await.is_err() {
            active.closer.close();
            return Err(ApiError::new(
                ErrorCode::PortsNotAllowed,
                "admission ended while the inside socket opened",
            ));
        }
        lock(&self.active).insert(id, active);
        Ok(tunnel)
    }
    /// The transport of a use that waits for its socket; none for an unknown or spent use.
    pub fn pending_transport(&self, id: Id) -> Option<PortTransport> {
        lock(&self.uses).get(&id).map(|usage| usage.transport)
    }

    /// Retires unused handles when an opening ends.
    pub fn close_opening(&self, opening: Id) {
        lock(&self.uses).retain(|_, u| u.opening != Some(opening));
        lock(&self.active).retain(|_, active| {
            if active.opening == Some(opening) {
                active.closer.close();
                false
            } else {
                true
            }
        });
    }
    /// Rechecks the exact active use before an inside program receives a fresh identity assertion.
    pub(crate) async fn peer_current(&self, connection: &PortConnection) -> ApiResult<bool> {
        let active = lock(&self.active).get(&connection.id).cloned();
        let Some(active) = active else {
            return Ok(false);
        };
        if active.closer.is_closed()
            || active.environment != connection.environment_id
            || active.generation != connection.generation
            || active.port != connection.port
            || active.transport != connection.transport
            || active.opening != connection.opening_id
        {
            return Ok(false);
        }
        match Self::current(&active).await {
            Ok(()) => Ok(!active.closer.is_closed()),
            Err(error)
                if matches!(
                    error.code(),
                    ErrorCode::Internal | ErrorCode::PortsUnavailable
                ) =>
            {
                Err(error)
            }
            Err(_) => Ok(false),
        }
    }

    async fn current(active: &Active) -> ApiResult<()> {
        if let Who::Person(identity) = &active.who {
            let mut tx = active.app.db.begin().await?;
            let env = crate::routes::ports::environment(&mut tx, active.environment).await?;
            if i64::from(env.generation) != active.generation {
                return Err(ApiError::new(
                    ErrorCode::PortsUnavailable,
                    "the runtime generation changed",
                ));
            }
            let principal = identity
                .principal(&mut tx, Time::new(active.app.clock.now()))
                .await?
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::PortsNotAllowed,
                        "the person can no longer use this port",
                    )
                })?;
            crate::routes::ports::may_use(
                &mut tx,
                &principal,
                &env,
                active.port,
                active.transport,
                active.opening,
            )
            .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(());
        }
        if matches!(active.who, Who::Link) {
            let mut tx = active.app.db.begin().await?;
            let env = crate::routes::ports::environment(&mut tx, active.environment).await?;
            if i64::from(env.generation) != active.generation {
                return Err(ApiError::new(
                    ErrorCode::PortsUnavailable,
                    "the runtime generation changed",
                ));
            }
            let row = orch_db::queries::ports::opening(
                &mut tx,
                active.opening.ok_or_else(ApiError::not_found)?,
            )
            .await?
            .ok_or_else(ApiError::not_found)?;
            crate::routes::ports::may_use_link(
                &mut tx,
                &env,
                &row,
                active.transport,
                Time::new(active.app.clock.now()),
            )
            .await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(());
        }
        let Who::Session(ctx) = &active.who else {
            return Err(ApiError::internal("a channel without a known admission"));
        };
        let (mut tx, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
        let env = crate::routes::ports::environment(&mut tx, active.environment).await?;
        if i64::from(env.generation) != active.generation {
            return Err(ApiError::new(
                ErrorCode::PortsUnavailable,
                "the runtime generation changed",
            ));
        }
        crate::routes::ports::may_use(
            &mut tx,
            guard.principal(),
            &env,
            active.port,
            active.transport,
            active.opening,
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
    /// Rechecks each current reader/credential independently of the opening's decider.
    pub async fn recheck(&self) {
        let now = lock(&self.uses)
            .values()
            .next()
            .map(|usage| Time::new(usage.ctx.app.clock.now()));
        if let Some(now) = now {
            lock(&self.uses).retain(|_, usage| usage.expires > now);
        }
        let active = lock(&self.active)
            .iter()
            .map(|(id, active)| (*id, active.clone()))
            .collect::<Vec<_>>();
        for (id, active) in active {
            if active.closer.is_closed() || Self::current(&active).await.is_err() {
                active.closer.close();
                lock(&self.active).remove(&id);
            }
        }
    }
    /// Keeps a single-use code that enters a browser into one port's stable name, for the
    /// ordinary short life of a use.
    pub fn enter_code(&self, entry: BrowserEntry, now: Time) -> String {
        let code = entry_token();
        let mut codes = lock(&self.codes);
        codes.retain(|_, kept| kept.expires > now);
        codes.insert(code.clone(), entry);
        code
    }

    /// Takes a code once, for the name it was made for and before it lapses.
    pub fn redeem_code(
        &self,
        code: &str,
        environment: Id,
        port: u16,
        now: Time,
    ) -> Option<BrowserEntry> {
        let entry = lock(&self.codes).remove(code)?;
        (entry.environment == environment && entry.port == port && entry.expires > now)
            .then_some(entry)
    }

    /// Remembers a browser's entry to one name and answers the token of its cookie.
    pub fn start_browser(&self, entry: BrowserEntry, now: Time) -> String {
        let token = entry_token();
        let mut browsers = lock(&self.browsers);
        browsers.retain(|_, kept| kept.expires > now);
        if browsers.len() >= BROWSER_ENTRIES_MAX
            && let Some(oldest) = browsers
                .iter()
                .min_by_key(|(_, kept)| kept.expires)
                .map(|(token, _)| token.clone())
        {
            browsers.remove(&oldest);
        }
        browsers.insert(token.clone(), entry);
        token
    }

    /// The browser's entry its cookie names, for this name, while it holds.
    pub fn browser(
        &self,
        token: &str,
        environment: Id,
        port: u16,
        now: Time,
    ) -> Option<BrowserEntry> {
        lock(&self.browsers)
            .get(token)
            .filter(|entry| {
                entry.environment == environment && entry.port == port && entry.expires > now
            })
            .cloned()
    }

    /// Opens a channel for a browser's request whose admission was asked just now, with who is
    /// calling as the ticket says; it is asked again at every tick, as every channel is.
    #[expect(
        clippy::too_many_arguments,
        reason = "the canonical use fields and current request are independently fenced"
    )]
    pub(crate) async fn open_browser(
        &self,
        app: &crate::state::App,
        env: &EnvironmentFull,
        port: u16,
        opening: Option<Id>,
        caller: PortCaller,
        identity: Option<RetainedIdentity>,
        revision: i64,
        outside: SocketAddr,
        id: Id,
    ) -> ApiResult<Tunnel> {
        let now = Time::new(app.clock.now());
        let expires = Time::new(now.get() + chrono::Duration::seconds(USE_SECONDS));
        let audience = if let Some(opening) = opening {
            let mut conn = app.db.acquire().await?;
            let row = orch_db::queries::ports::opening(&mut conn, opening)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Some(orch_engine::port_capabilities::kind_of(&row.kind))
        } else {
            None
        };
        let claims = orch_link_contract::ports::PortTicketClaims {
            environment_id: env.id,
            generation: i64::from(env.generation),
            port,
            transport: PortTransport::Tcp,
            opening_id: opening,
            audience,
            authority_revision: revision,
            allocation_revision: None,
            host_template: None,
            source: PortConnectionOutside {
                address: outside.ip().to_string(),
                port: outside.port(),
                caller,
            },
        };
        let machine = env
            .machine_id
            .ok_or_else(|| ApiError::new(ErrorCode::PortsUnavailable, "the connector is absent"))?;
        let ticket = app
            .link
            .tickets()
            .port_use(id, machine, claims, expires)
            .map_err(ApiError::internal)?;
        let tunnel = app
            .link
            .terminals()
            .open(machine, &ticket)
            .await
            .map_err(|_| {
                ApiError::new(
                    ErrorCode::PortsNotListening,
                    "the current owned target could not open",
                )
            })?;
        let active = Active {
            app: app.clone(),
            who: identity.map_or(Who::Link, Who::Person),
            environment: env.id,
            generation: i64::from(env.generation),
            port,
            transport: PortTransport::Tcp,
            opening,
            closer: tunnel.closer(),
        };
        if Self::current(&active).await.is_err() {
            active.closer.close();
            return Err(ApiError::new(
                ErrorCode::PortsNotAllowed,
                "admission ended while the inside socket opened",
            ));
        }
        lock(&self.active).insert(id, active);
        Ok(tunnel)
    }

    /// Retires every transient handle when this API shuts down.
    pub fn clear(&self) {
        lock(&self.codes).clear();
        lock(&self.browsers).clear();
        lock(&self.uses).clear();
        lock(&self.watch_queries).clear();
        for active in lock(&self.active).drain().map(|(_, a)| a) {
            active.closer.close();
        }
    }
}

/// The authority-changing requests that only sign their caller in or keep bytes they send: they make
/// the caller's own session, token or upload and end nobody's admission. They are by far the most
/// frequent of those requests, so no reconciliation follows them.
const OWN_SIGN_IN_OR_BYTES: &[&str] = &[
    "SetupCreate",
    "AuthRegister",
    "AuthLogin",
    "AuthOidcCallback",
    "AuthRefresh",
    "AuthEmailVerify",
    "UploadCreate",
    "UploadSessionComplete",
    "ChatDraftSet",
];

/// Whether a request that changes who may do what can have ended a saved port admission: whatever
/// takes a role, an entry, a membership, a policy, a place or an environment away can.
#[must_use]
pub fn may_end_admission(route: &str) -> bool {
    !OWN_SIGN_IN_OR_BYTES.contains(&route)
}

/// Closes every saved port admission whose maker's or approver's authority, policy, named place,
/// requester or environment has ended, each environment in a transaction of its own under the
/// exclusive authority lock, and retires the live channels of what it closed. A request that changed
/// authority runs this after its own transaction committed and before it answers, so that no caller
/// who saw the change answered can still find the admission it ended; a revocation followed by a
/// restoration therefore still ends the admission, as the opening's lifetime requires.
pub async fn close_ended(app: &crate::state::App) {
    let found = async {
        let mut conn = app.db.acquire().await?;
        if !orch_db::queries::ports::installed(&mut conn).await? {
            return Ok::<Vec<Id>, orch_db::DbError>(Vec::new());
        }
        orch_db::queries::ports::admitted_environments(&mut conn).await
    }
    .await;
    let environments = match found {
        Ok(environments) => environments,
        Err(error) => {
            tracing::warn!(%error, "the saved port admissions could not be listed");
            return;
        }
    };
    for id in environments {
        let reconciled = async {
            let mut tx = app.db.begin().await?;
            orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
            let closed =
                orch_engine::ports::reconcile(&mut tx, id, Time::new(app.clock.now())).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok::<_, orch_engine::ports::Error>(closed)
        }
        .await;
        match reconciled {
            Ok(closed) => retire(app, &closed),
            Err(error) => {
                tracing::warn!(%error, environment = %id, "a saved port admission could not be reconciled");
            }
        }
    }
}

/// Retires the unused handles and live channels of closed openings on this API process.
fn retire(app: &crate::state::App, closed: &[Id]) {
    for opening in closed {
        app.cache.ports().close_opening(*opening);
        app.link.terminals().close_port_opening(*opening);
    }
    // A host opening's listeners are its node's: it is told to end them.
    crate::host_ports::unpublish_in_background(app, closed);
}

/// The authorization epoch now: it moves with every committed change of who may do what.
async fn current_epoch(app: &crate::state::App) -> Result<i64, orch_db::DbError> {
    let mut conn = app.db.acquire().await?;
    orch_db::queries::access::epoch(&mut conn).await
}

/// Reconciles saved admission and owned activity without a human subscriber or starting a unit.
pub fn start(
    app: &crate::state::App,
    stop: &tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    crate::port_tools::install(app);
    let app = std::sync::Arc::clone(app);
    let stop = stop.clone();
    // The held host openings are published again with this process's tickets, and from time to time.
    tokio::spawn(crate::host_ports::keep_published(
        std::sync::Arc::clone(&app),
        stop.clone(),
    ));
    let outcomes = app.clone();
    let outcome_stop = stop.clone();
    tokio::spawn(async move {
        // A one-second look is for late delivery only; the caller itself checks at 250 ms.
        let mut look = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! { () = outcome_stop.cancelled() => return, _ = look.tick() => {} }
            if let Err(error) = crate::port_tools::tell_late(&outcomes).await {
                tracing::warn!(%error, "a late port decision could not be delivered");
            }
        }
    });
    tokio::spawn(async move {
        // Authority is checked at the carrier's quarter-second rhythm; native
        // observation and activity use the ordinary five-second watch cadence.
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut observed_at = tokio::time::Instant::now() - Duration::from_secs(5);
        let mut epoch_seen: Option<i64> = None;
        loop {
            tokio::select! {()=stop.cancelled()=>break,_=tick.tick()=>{}}
            app.cache.ports().recheck().await;
            app.link.terminals().prune_ports();
            // A change of who may do what made outside a request (an agent's tool, a job, an
            // ended membership) closes what it ended within this tick, as a request's own pass
            // does before it answers.
            if let Ok(epoch) = current_epoch(&app).await {
                if epoch_seen.is_some_and(|seen| seen != epoch) {
                    close_ended(&app).await;
                }
                epoch_seen = Some(epoch);
            }
            if observed_at.elapsed() < Duration::from_secs(5) {
                continue;
            }
            observed_at = tokio::time::Instant::now();
            let found = async {
                let mut conn = app.db.acquire().await?;
                if !orch_db::queries::ports::installed(&mut conn).await? {
                    return Ok::<Vec<Id>, orch_db::DbError>(Vec::new());
                }
                orch_db::queries::ports::admitted_environments(&mut conn).await
            }
            .await;
            let Ok(mut environments) = found else {
                continue;
            };
            environments.extend(
                lock(&app.cache.ports().active)
                    .values()
                    .map(|active| active.environment),
            );
            environments.sort();
            environments.dedup();
            for id in environments {
                let reconciled = async {
                    let mut tx = app.db.begin().await?;
                    orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
                    let closed =
                        orch_engine::ports::reconcile(&mut tx, id, Time::new(app.clock.now()))
                            .await?;
                    let env = orch_db::queries::environments::routes::environments(&mut tx, &[id])
                        .await?
                        .into_iter()
                        .next();
                    if let Some(env) = &env
                        && let Some(snapshot) =
                            app.cache.ports().snapshot(id, i64::from(env.generation))
                    {
                        if let Some(at) = snapshot.activity_at {
                            orch_db::queries::ports::activity(
                                &mut tx,
                                id,
                                snapshot.generation,
                                at.get(),
                            )
                            .await?;
                        }
                        for row in orch_db::queries::ports::openings(&mut tx, id).await? {
                            if matches!(row.state.as_str(), "waiting" | "refused" | "closed")
                                || row.kind == "host"
                            {
                                continue;
                            }
                            let available = snapshot.discovery == PortDiscovery::Native
                                && snapshot.listeners.iter().any(|l| {
                                    i32::from(l.port) == row.port
                                        && row.transports.iter().any(|transport| {
                                            transport
                                                == match l.transport {
                                                    PortTransport::Tcp => "tcp",
                                                    PortTransport::Udp => "udp",
                                                }
                                        })
                                        && !l.internal
                                        && l.unavailable_reason.is_none()
                                });
                            let state = if env.status != "running" {
                                "stopped"
                            } else if snapshot.discovery != PortDiscovery::Native {
                                "unavailable"
                            } else if available {
                                "open"
                            } else {
                                "not_listening"
                            };
                            orch_db::queries::ports::availability(
                                &mut tx,
                                row.id,
                                state,
                                snapshot.reason.as_deref(),
                            )
                            .await?;
                        }
                    }
                    tx.commit().await.map_err(orch_db::DbError::from)?;
                    Ok::<_, orch_engine::ports::Error>((closed, env))
                }
                .await;
                if let Ok((closed, env)) = reconciled {
                    retire(&app, &closed);
                    if let Some(env) = env
                        && env.status == "running"
                        && let Some(machine) = env.machine_id
                        && app.link.is_online(machine)
                    {
                        let request_ref = orch_types::new_id();
                        {
                            let mut pending = lock(&app.cache.ports().watch_queries);
                            pending.retain(|_, (_, id, _)| *id != env.id);
                            pending
                                .insert(request_ref, (machine, env.id, i64::from(env.generation)));
                        }
                        let body = orch_link_contract::ports::PortsLook {
                            environment_id: env.id,
                            generation: i64::from(env.generation),
                            request_ref,
                        };
                        let _ = app
                            .link
                            .dispatcher()
                            .issue(
                                machine,
                                &orch_link_contract::commands::SerialKey::Environment(env.id),
                                &body,
                            )
                            .await;
                    }
                }
            }
        }
        app.cache.ports().clear();
    })
}
