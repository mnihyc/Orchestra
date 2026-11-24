//! The human check: before a person without a session signs in, registers, or has mail sent to an address, the system
//! may ask them to prove they are not a program.
//!
//! What a vendor says of a token is the backends' ([`orch_human_check_contract::HumanCheckProvider`]); whether the
//! person passes is decided once, in the contract ([`orch_human_check_contract::judge`]). This module is what is
//! left to the API: which acts are guarded (the system's settings), where the secret is kept (sealed, never read
//! back), the count that bounds how often one address may make this server call a vendor, the answers, and what is
//! recorded when the vendor cannot be asked.
//!
//! The check sits in front of a name's counts and proofs, never after or among them: a token that fails costs the
//! name no wrong try, so a stranger cannot lock a name by failing checks, and what is answered does not depend on
//! whether the name exists. Nothing is held while the vendor is asked: no database connection and no transaction.
//! Where no check is switched on, nothing here reads the vendor, the secret or the request's token.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration as StdDuration, Instant};

use chrono::Duration;
use orch_access_contract::audit::AuditAction;
use orch_api_contract::admin::{
    HumanCheckEdge, HumanCheckEdgeState, HumanCheckEdgeVia, HumanCheckTestReply,
};
use orch_api_contract::auth::{
    HumanCheckAct, HumanCheckService, HumanCheckVerification, HumanCheckWidget, MetaHumanCheck,
};
use orch_api_contract::common::{ErrorCode, HUMAN_CHECK_HEADER, HUMAN_CHECK_UNLOADED};
use orch_db::Conn;
use orch_db::audit::Actor;
use orch_db::queries::identity::{Bound, Reservation, refund_try, reserve_try};
pub use orch_human_check_contract::Act;
use orch_human_check_contract::edge::{EdgeReader, lacking};
use orch_human_check_contract::{
    Config, Expectation, Failure, HCAPTCHA, HumanCheckProvider, RECAPTCHA, Request, Secret,
    Unreachable, Verdict, Why, hosts_of, judge,
};
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::registry::{self, keys};
use orch_types::Id;
use serde_json::json;

use super::{record, setting, throttle};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::resources::sealing::{binding, master_key};

/// What kind of owner the sealed secret of the human check has: the system's settings own it, as no node does.
pub const SECRET_OWNER: &str = "setting";
/// The name under which the secret is kept.
pub const SECRET_NAME: &str = "HUMAN_CHECK_SECRET";

/// What owns the secret among the system's settings: no node, so the nil identifier.
#[must_use]
pub fn secret_owner_id() -> Id {
    Id::nil()
}

/// The word of a token that is not one: what the test of the service sends.
const TEST_TOKEN: &str = "orch-test-token";

/// The backends this server can ask, by the service and the way each speaks for.
static PROVIDERS: LazyLock<RwLock<Vec<Arc<dyn HumanCheckProvider>>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Makes `providers` the backends this server can ask, in place of any other.
pub fn install_providers(providers: Vec<Arc<dyn HumanCheckProvider>>) {
    if let Ok(mut installed) = PROVIDERS.write() {
        *installed = providers;
    }
}

/// The backend that speaks for `service` in the way named, where one is installed.
fn provider_for(service: &str, way: &str) -> Option<Arc<dyn HumanCheckProvider>> {
    PROVIDERS.read().ok().and_then(|installed| {
        installed
            .iter()
            .find(|provider| {
                provider.service() == service
                    && (provider.way().is_empty() || provider.way() == way)
            })
            .cloned()
    })
}

/// The way this server looks at its own web edge: through this reader, which a test scripts.
static EDGE_READER: LazyLock<RwLock<Option<Arc<dyn EdgeReader>>>> =
    LazyLock::new(|| RwLock::new(None));

/// Makes `reader` the way this server looks at its own web edge.
pub fn install_edge_reader(reader: Arc<dyn EdgeReader>) {
    if let Ok(mut installed) = EDGE_READER.write() {
        *installed = Some(reader);
    }
}

/// The address of the stack's own web edge, as the services on the stack's network know it.
const STACK_EDGE: &str = "http://frontend/";

/// Where the server read the web edge's policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// At the address people open, through any front of the operator's own: what a browser is given.
    Public,
    /// At the stack's own web edge, where a front of the operator's own is not seen.
    Stack,
}

/// What the web edge's content policy leaves a page of the service that is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edge {
    /// The policy names the hosts the widget needs.
    Allows(Via),
    /// The policy leaves some out: the browser blocks the widget, and nobody could have a token to give.
    Blocks {
        /// Where the policy was read.
        via: Via,
        /// What it leaves out, as `<host> in <directive>`.
        lacking: Vec<String>,
    },
    /// The policy could not be read, so nothing is known.
    Unknown,
}

impl Edge {
    /// Whether the policy is known to block the widget.
    #[must_use]
    pub fn blocks(&self) -> bool {
        matches!(self, Edge::Blocks { .. })
    }

    /// What an administrator is told of it.
    fn told(&self) -> HumanCheckEdge {
        let via = |via: Via| match via {
            Via::Public => HumanCheckEdgeVia::Public,
            Via::Stack => HumanCheckEdgeVia::Stack,
        };
        match self {
            Edge::Allows(seen) => HumanCheckEdge {
                state: HumanCheckEdgeState::Allows,
                via: Some(via(*seen)),
                lacking: Vec::new(),
            },
            Edge::Blocks { via: seen, lacking } => HumanCheckEdge {
                state: HumanCheckEdgeState::Blocks,
                via: Some(via(*seen)),
                lacking: lacking.clone(),
            },
            Edge::Unknown => HumanCheckEdge {
                state: HumanCheckEdgeState::Unknown,
                via: None,
                lacking: Vec::new(),
            },
        }
    }
}

/// What was last found of the web edge for each service and address it was asked for, and when.
static EDGE_SEEN: LazyLock<tokio::sync::Mutex<BTreeMap<String, (Instant, Edge)>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(BTreeMap::new()));

struct ServiceObservation {
    key: String,
    checked_at: Instant,
    unreachable: Option<Unreachable>,
}

/// What was last found of whether the service answers, for the keys it was asked with, and when.
static SERVICE_SEEN: LazyLock<tokio::sync::Mutex<Option<ServiceObservation>>> =
    LazyLock::new(|| tokio::sync::Mutex::new(None));

/// Forgets what was found of the web edge and of the service, so that the next look is a new one.
pub async fn forget_looks() {
    EDGE_SEEN.lock().await.clear();
    *SERVICE_SEEN.lock().await = None;
}

/// What a look at the web edge is for: the service, and the address it is loaded from.
fn edge_key(policy: &Policy) -> String {
    format!(
        "{}|{}",
        policy.service,
        policy.service == RECAPTCHA && policy.alternate_host
    )
}

/// Reads the web edge's content policy where a browser is given it, and says what it leaves the service. A front of
/// the operator's own that sets or changes the policy is seen only through the address people open; the stack's own
/// edge is looked at where that cannot be reached, and says what the edge itself sends.
async fn probe(public_url: &str, policy: &Policy) -> Edge {
    let reader = EDGE_READER
        .read()
        .ok()
        .and_then(|installed| installed.clone());
    let Some(reader) = reader else {
        return Edge::Unknown;
    };
    let Some(provider) = provider_for(&policy.service, &policy.verification) else {
        return Edge::Unknown;
    };
    let hosts = provider.edge_hosts(policy.alternate_host);
    let public = format!("{}/", public_url.trim_end_matches('/'));
    let (public_answer, stack_answer) = tokio::join!(
        reader.read(&public, policy.timeout),
        reader.read(STACK_EDGE, policy.timeout)
    );
    let (via, answer) = match (public_answer, stack_answer) {
        (Ok(answer), _) if answer.status < 500 => (Via::Public, answer),
        (_, Ok(answer)) if answer.status < 500 => (Via::Stack, answer),
        _ => return Edge::Unknown,
    };
    let missing = lacking(&answer.policies, &hosts);
    if missing.is_empty() {
        Edge::Allows(via)
    } else {
        Edge::Blocks {
            via,
            lacking: missing.iter().map(ToString::to_string).collect(),
        }
    }
}

/// What was last found of the web edge for `key`, and whether it was within `look`.
async fn found(key: &str, look: StdDuration) -> Option<(bool, Edge)> {
    let seen = EDGE_SEEN.lock().await;
    seen.get(key)
        .map(|(at, edge)| (at.elapsed() < look, edge.clone()))
}

/// What the web edge's policy leaves the service chosen: what was found within the last `look`, or a new look, and
/// always a new one where `fresh`. Nothing is held while the edge is asked.
pub async fn edge_of(public_url: &str, policy: &Policy, fresh: bool) -> Edge {
    if policy.service.is_empty() {
        return Edge::Unknown;
    }
    let key = edge_key(policy);
    if !fresh && let Some((true, edge)) = found(&key, policy.look).await {
        return edge;
    }
    let edge = probe(public_url, policy).await;
    EDGE_SEEN
        .lock()
        .await
        .insert(key, (Instant::now(), edge.clone()));
    edge
}

/// Whether a look at the web edge is being made in the background, so that one at a time is.
static LOOKING: AtomicBool = AtomicBool::new(false);

/// What the web edge's policy leaves the service, for a person who is waiting at a form: what was last found, and a
/// new look in the background where one is due, so that nobody waits on a slow edge more than the once, the first
/// time anything is asked.
async fn edge_for_request(public_url: &str, policy: &Policy) -> Edge {
    if policy.service.is_empty() {
        return Edge::Unknown;
    }
    let key = edge_key(policy);
    let stale = match found(&key, policy.look).await {
        Some((true, edge)) => return edge,
        Some((false, edge)) => edge,
        None => return edge_of(public_url, policy, false).await,
    };
    if LOOKING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let (address, policy) = (public_url.to_owned(), policy.clone());
        tokio::spawn(async move {
            edge_of(&address, &policy, true).await;
            LOOKING.store(false, Ordering::Release);
        });
    }
    stale
}

/// What the web edge's policy leaves the service chosen, as an administrator is told it, from a new look; none while
/// no service is chosen.
pub async fn edge_state(public_url: &str, policy: &Policy) -> Option<HumanCheckEdge> {
    if policy.service.is_empty() {
        return None;
    }
    Some(edge_of(public_url, policy, true).await.told())
}

/// What happens to a person when the service cannot be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhenUnreachable {
    /// They are let in, and the event is recorded.
    Admit,
    /// They are turned away until the service can be asked.
    Refuse,
}

/// What the system's settings say of the human check.
#[derive(Debug, Clone)]
pub struct Policy {
    /// The service whose widget the pages show; empty where none is chosen.
    pub service: String,
    /// The public key of its widget.
    pub site_key: String,
    /// The Google Cloud project, for the way that needs one.
    pub project_id: String,
    /// The way a service with several is asked.
    pub verification: String,
    /// How the widget shows itself.
    pub widget: HumanCheckWidget,
    /// The acts that need a check.
    pub acts: Vec<Act>,
    /// The least score that passes, in whole percent.
    pub least_score_percent: u8,
    /// What happens when the service cannot be asked.
    pub when_unreachable: WhenUnreachable,
    /// Whether the widget is loaded from the service's alternate address.
    pub alternate_host: bool,
    /// How long one call to the service may take.
    pub timeout: StdDuration,
    /// How long a page waits for the widget, in seconds.
    pub page_wait_secs: u32,
    /// How often one address may make a call.
    pub address: Bound,
    /// How long what was found of the web edge and of the service is remembered.
    pub look: StdDuration,
}

impl Policy {
    /// Whether a check is switched on and has what its page needs: a service and the key of its widget. A choice with
    /// no site key asks nobody, so that a half-made setting cannot shut a page.
    #[must_use]
    pub fn active(&self) -> bool {
        !self.service.is_empty() && !self.site_key.trim().is_empty()
    }

    /// Whether `act` needs a check.
    #[must_use]
    pub fn guards(&self, act: Act) -> bool {
        self.active() && self.acts.contains(&act)
    }

    /// What the pages are told: nothing where no check is on.
    #[must_use]
    pub fn shown(&self) -> Option<MetaHumanCheck> {
        if !self.active() {
            return None;
        }
        let recaptcha = self.service == RECAPTCHA;
        let provider = match self.service.as_str() {
            HCAPTCHA => HumanCheckService::Hcaptcha,
            RECAPTCHA => HumanCheckService::Recaptcha,
            _ => return None,
        };
        Some(MetaHumanCheck {
            provider,
            site_key: self.site_key.trim().to_owned(),
            // reCAPTCHA scores without being seen: it has no widget to tick.
            mode: if recaptcha {
                HumanCheckWidget::Invisible
            } else {
                self.widget
            },
            acts: self.acts.iter().copied().map(shown_act).collect(),
            alternate_host: recaptcha && self.alternate_host,
            wait_secs: self.page_wait_secs,
            verification: recaptcha.then(|| {
                if self.verification == "assessments" {
                    HumanCheckVerification::Assessments
                } else {
                    HumanCheckVerification::Siteverify
                }
            }),
        })
    }
}

fn shown_act(act: Act) -> HumanCheckAct {
    match act {
        Act::SignIn => HumanCheckAct::SignIn,
        Act::EmailCode => HumanCheckAct::EmailCode,
        Act::Register => HumanCheckAct::Register,
        Act::Recovery => HumanCheckAct::Recovery,
        Act::Resend => HumanCheckAct::Resend,
    }
}

/// Reads what the system's settings say of the human check.
pub async fn policy(conn: &mut Conn) -> ApiResult<Policy> {
    let defs = [
        keys::HUMAN_CHECK_PROVIDER,
        keys::HUMAN_CHECK_SITE_KEY,
        keys::HUMAN_CHECK_PROJECT_ID,
        keys::HUMAN_CHECK_VERIFICATION,
        keys::HUMAN_CHECK_MODE,
        keys::HUMAN_CHECK_ACTS,
        keys::HUMAN_CHECK_MIN_SCORE,
        keys::HUMAN_CHECK_WHEN_UNREACHABLE,
        keys::HUMAN_CHECK_ALTERNATE_HOST,
        keys::HUMAN_CHECK_TIMEOUT_SECS,
        keys::HUMAN_CHECK_PAGE_WAIT_SECS,
        keys::HUMAN_CHECK_ADDRESS_LIMIT,
        keys::HUMAN_CHECK_ADDRESS_WINDOW_MINUTES,
        keys::HUMAN_CHECK_LOOK_SECS,
    ]
    .map(registry::get);
    let defs: Vec<_> = defs.into_iter().flatten().collect();
    let snapshot = Box::pin(Snapshot::load(conn, Place::System, Wanted::Keys(&defs)))
        .await
        .map_err(ApiError::internal)?;
    let text = |key: &str| -> ApiResult<String> {
        Ok(setting::<String>(&snapshot, key)?.unwrap_or_default())
    };
    let whole = |key: &str, default: i64| -> ApiResult<i64> {
        Ok(setting::<i64>(&snapshot, key)?.unwrap_or(default))
    };
    let provider = text(keys::HUMAN_CHECK_PROVIDER)?;
    let service = if provider == "off" {
        String::new()
    } else {
        provider
    };
    let acts: Vec<String> = setting(&snapshot, keys::HUMAN_CHECK_ACTS)?.unwrap_or_default();
    Ok(Policy {
        service,
        site_key: text(keys::HUMAN_CHECK_SITE_KEY)?,
        project_id: text(keys::HUMAN_CHECK_PROJECT_ID)?,
        verification: text(keys::HUMAN_CHECK_VERIFICATION)?,
        widget: if text(keys::HUMAN_CHECK_MODE)? == "invisible" {
            HumanCheckWidget::Invisible
        } else {
            HumanCheckWidget::Checkbox
        },
        acts: acts.iter().filter_map(|word| Act::named(word)).collect(),
        least_score_percent: u8::try_from(whole(keys::HUMAN_CHECK_MIN_SCORE, 50)?.clamp(0, 100))
            .unwrap_or(50),
        when_unreachable: if text(keys::HUMAN_CHECK_WHEN_UNREACHABLE)? == "refuse" {
            WhenUnreachable::Refuse
        } else {
            WhenUnreachable::Admit
        },
        alternate_host: setting::<bool>(&snapshot, keys::HUMAN_CHECK_ALTERNATE_HOST)?
            .unwrap_or(false),
        timeout: StdDuration::from_secs(
            u64::try_from(whole(keys::HUMAN_CHECK_TIMEOUT_SECS, 5)?.max(1)).unwrap_or(5),
        ),
        page_wait_secs: u32::try_from(whole(keys::HUMAN_CHECK_PAGE_WAIT_SECS, 20)?.max(1))
            .unwrap_or(u32::MAX),
        address: Bound {
            limit: u32::try_from(whole(keys::HUMAN_CHECK_ADDRESS_LIMIT, 30)?.max(1))
                .unwrap_or(u32::MAX),
            window: Duration::minutes(whole(keys::HUMAN_CHECK_ADDRESS_WINDOW_MINUTES, 15)?.max(1)),
            gap: None,
        },
        look: StdDuration::from_secs(
            u64::try_from(whole(keys::HUMAN_CHECK_LOOK_SECS, 60)?.max(1)).unwrap_or(60),
        ),
    })
}

/// The token a request carries, where it carries one.
fn token_of(ctx: &Ctx) -> Option<String> {
    ctx.headers
        .get(HUMAN_CHECK_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

/// Whether the request is a signed-in person's: it carries an access token that is good now.
async fn has_session(ctx: &Ctx) -> bool {
    use crate::middleware::authentication::{Caller, authenticate};
    ctx.headers.contains_key(http::header::AUTHORIZATION)
        && matches!(
            authenticate(&ctx.app, &ctx.headers, ctx.now).await,
            Ok(Caller::User(_))
        )
}

/// The secret of the service, opened for one call; none where none is kept.
pub async fn open_secret(conn: &mut Conn) -> ApiResult<Option<Secret>> {
    let Some(row) =
        orch_db::queries::settings::secret(conn, SECRET_OWNER, secret_owner_id(), SECRET_NAME)
            .await?
    else {
        return Ok(None);
    };
    let plain = master_key()?
        .open(
            &row.sealed,
            &binding(SECRET_OWNER, secret_owner_id(), SECRET_NAME),
        )
        .map_err(|_| ApiError::internal("the secret of the human check could not be opened"))?;
    let text = String::from_utf8(plain)
        .map_err(|_| ApiError::internal("the secret of the human check is not text"))?;
    Ok(Some(Secret::new(text)))
}

/// What the answer of a person who did not pass says.
fn failed(failure: Failure) -> ApiError {
    match failure {
        Failure::Missing => ApiError::new(
            ErrorCode::HumanCheckRequired,
            "a human check is needed for this",
        ),
        Failure::Expired | Failure::Replayed => ApiError::new(
            ErrorCode::HumanCheckExpired,
            "the human check has run out or was used already",
        )
        .with("reason", failure.reason()),
        Failure::Invalid | Failure::WrongAct | Failure::WrongSite | Failure::LowScore => {
            ApiError::new(
                ErrorCode::HumanCheckFailed,
                "the human check was not passed",
            )
            .with("reason", failure.reason())
        }
    }
}

/// What the answer of a person turned away because the service could not be asked says.
fn unavailable() -> ApiError {
    ApiError::new(
        ErrorCode::HumanCheckUnavailable,
        "the human check cannot be reached",
    )
}

/// Asks the person for the human check that `act` needs, if the system asks one: the token of the request is
/// verified with the service, and the person passes, or is refused. Without a check switched on for `act` it does
/// nothing at all.
///
/// Nothing is held while the service is asked: this takes its own short connections.
pub async fn require(ctx: &Ctx, act: Act) -> ApiResult<()> {
    let policy = {
        let mut conn = ctx.app.db.acquire().await?;
        policy(&mut conn).await?
    };
    if !policy.guards(act) {
        return Ok(());
    }
    // A person who is signed in asks for their own confirmation link again from their profile, with no widget in
    // reach: that one act is not asked of them. Every other act is, whatever the request carries, since a token of
    // any account would otherwise open the sign-in of every other.
    if act == Act::Resend && has_session(ctx).await {
        return Ok(());
    }
    // Where the web edge's policy has no room for the service, no browser can load its widget and nobody has a token
    // to give: the service is as good as unreachable, and the setting says whether people are let in or turned away.
    // The server looks at its own edge for itself and takes nobody's word for it.
    if edge_for_request(&ctx.app.config.system.public_url, &policy)
        .await
        .blocks()
    {
        let why = Unreachable::new(
            Why::Edge,
            "the web edge's content policy has no room for the service",
        );
        return unreached(ctx, &policy, act, &why).await;
    }
    let Some(token) = token_of(ctx) else {
        return Err(failed(Failure::Missing));
    };
    // A page whose widget did not load in its browser says so instead of giving a token. Anybody can say it, so it
    // passes nothing by itself: it is looked into, and it counts as an unreachable service only where the server
    // cannot ask the service either. Where the service answers, the person is asked for a check as before.
    if token == HUMAN_CHECK_UNLOADED {
        return match service_unreached(ctx, &policy).await? {
            Some(why) => unreached(ctx, &policy, act, &why).await,
            None => Err(failed(Failure::Missing)),
        };
    }
    // A try is counted before the service is asked, and given back if it passes: tries made at once would all pass
    // the limit before the first of them was counted.
    let bucket = ctx.remote.map(|ip| format!("human|{ip}"));
    if let Some(bucket) = &bucket {
        let mut conn = ctx.app.db.acquire().await?;
        match reserve_try(&mut conn, bucket, ctx.now, &policy.address).await? {
            Reservation::Counted { .. } => {}
            Reservation::Refused { retry_after } => {
                return Err(throttle::rate_limited(retry_after));
            }
        }
    }
    let verdict = ask(ctx, &policy, act, &token).await?;
    match verdict {
        Verdict::Pass => {
            if let Some(bucket) = &bucket {
                let mut conn = ctx.app.db.acquire().await?;
                refund_try(&mut conn, bucket, ctx.now).await?;
            }
            Ok(())
        }
        Verdict::Fail(failure) => Err(failed(failure)),
        Verdict::Unavailable(why) => unreached(ctx, &policy, act, &why).await,
    }
}

/// What happens to a person when the service could not be asked: it is recorded, and they are let in or turned away.
async fn unreached(ctx: &Ctx, policy: &Policy, act: Act, why: &Unreachable) -> ApiResult<()> {
    tracing::warn!(
        act = act.as_str(),
        why = why.why.word(),
        "the human check could not be asked"
    );
    note_unreachable(ctx, policy, act, why).await;
    match policy.when_unreachable {
        WhenUnreachable::Admit => Ok(()),
        WhenUnreachable::Refuse => Err(unavailable()),
    }
}

/// Whether the service can be asked at all, from what this server finds with a token that is not one, and not asked
/// again within `look`: how it could not be, or none where it answered (it refused the token, as it should).
async fn service_unreached(ctx: &Ctx, policy: &Policy) -> ApiResult<Option<Unreachable>> {
    let key = format!(
        "{}|{}|{}|{}",
        policy.service, policy.verification, policy.site_key, policy.project_id
    );
    let mut seen = SERVICE_SEEN.lock().await;
    if let Some(observation) = seen.as_ref()
        && observation.key == key
        && observation.checked_at.elapsed() < policy.look
    {
        return Ok(observation.unreachable.clone());
    }
    let answer = match ask_as(ctx, policy, Act::SignIn, TEST_TOKEN, None, None).await? {
        Verdict::Unavailable(why) => Some(why),
        Verdict::Pass | Verdict::Fail(_) => None,
    };
    *seen = Some(ServiceObservation {
        key,
        checked_at: Instant::now(),
        unreachable: answer.clone(),
    });
    Ok(answer)
}

/// Asks the service about `token` and decides.
async fn ask(ctx: &Ctx, policy: &Policy, act: Act, token: &str) -> ApiResult<Verdict> {
    let agent = super::user_agent(ctx);
    ask_as(ctx, policy, act, token, ctx.remote, agent.as_deref()).await
}

/// Asks the service about `token`, saying what it is told of the person who gave it, and decides.
async fn ask_as(
    ctx: &Ctx,
    policy: &Policy,
    act: Act,
    token: &str,
    remote_ip: Option<std::net::IpAddr>,
    user_agent: Option<&str>,
) -> ApiResult<Verdict> {
    let Some(provider) = provider_for(&policy.service, &policy.verification) else {
        return Ok(Verdict::Unavailable(Unreachable::new(
            Why::Setup,
            "this server has no way to ask the service",
        )));
    };
    let secret = {
        let mut conn = ctx.app.db.acquire().await?;
        open_secret(&mut conn).await?
    };
    let Some(secret) = secret else {
        return Ok(Verdict::Unavailable(Unreachable::new(
            Why::Secret,
            "no secret is kept for the service",
        )));
    };
    let config = Config {
        site_key: policy.site_key.trim().to_owned(),
        project_id: policy.project_id.trim().to_owned(),
        secret,
        timeout: policy.timeout,
    };
    let answer = provider
        .verify(&Request {
            token,
            act,
            remote_ip,
            user_agent,
            config: &config,
        })
        .await;
    let hosts = hosts_of(
        std::iter::once(ctx.app.config.system.public_url.as_str()).chain(
            ctx.app
                .config
                .system
                .alternate_urls
                .iter()
                .map(String::as_str),
        ),
    );
    Ok(judge(
        provider.traits(),
        answer,
        &Expectation {
            act,
            hosts: &hosts,
            least_score_percent: policy.least_score_percent,
        },
    ))
}

/// How many events of each act and outcome this process has seen since it last wrote their row.
static UNREACHED: LazyLock<Mutex<BTreeMap<(&'static str, &'static str), i64>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// Records that the service could not be asked: at most one row in an hour for an act and an outcome, with how many
/// times it happened since the row before. A failure to write it is logged and never fails the request.
async fn note_unreachable(ctx: &Ctx, policy: &Policy, act: Act, why: &Unreachable) {
    let (action, outcome) = match policy.when_unreachable {
        WhenUnreachable::Admit => (
            orch_db::audit::action!(AuditAction::AuthHumanCheckAdmitted),
            "admitted",
        ),
        WhenUnreachable::Refuse => (
            orch_db::audit::action!(AuditAction::AuthHumanCheckRefused),
            "refused",
        ),
    };
    if let Ok(mut seen) = UNREACHED.lock() {
        *seen.entry((act.as_str(), outcome)).or_insert(0) += 1;
    }
    if let Err(error) = write_unreachable(ctx, action, outcome, act, why).await {
        tracing::warn!(
            ?error,
            "the audit line of the human check could not be written"
        );
    }
}

async fn write_unreachable(
    ctx: &Ctx,
    action: AuditAction,
    outcome: &'static str,
    act: Act,
    why: &Unreachable,
) -> ApiResult<()> {
    let mut tx = ctx.app.db.begin().await?;
    let latest =
        orch_db::queries::human_check::lock_and_latest(&mut tx, action.as_str(), act.as_str())
            .await?;
    if latest.is_some_and(|at| ctx.now - at < orch_db::queries::human_check::AUDIT_INTERVAL) {
        return Ok(());
    }
    let count = UNREACHED
        .lock()
        .ok()
        .and_then(|mut seen| seen.remove(&(act.as_str(), outcome)))
        .unwrap_or(1);
    let summary = if outcome == "admitted" {
        "People were let in without a human check, because the service could not be asked"
    } else {
        "People were turned away, because the service of the human check could not be asked"
    };
    record(
        ctx,
        &mut tx,
        Actor::System,
        action,
        None,
        summary,
        json!({
            "act": act.as_str(),
            "why": why.why.word(),
            "detail": why.detail,
            "count": count,
        }),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Asks the service with a token that is not one, to see that it is reached and accepts the keys: a service that
/// complains of the token and of nothing else is working.
pub async fn test(ctx: &Ctx) -> ApiResult<HumanCheckTestReply> {
    let not_ok = |error: String| HumanCheckTestReply {
        ok: false,
        error: Some(error),
    };
    let policy = {
        let mut conn = ctx.app.db.acquire().await?;
        policy(&mut conn).await?
    };
    if !policy.active() {
        return Ok(not_ok(
            "No human check is switched on, or it has no site key.".to_owned(),
        ));
    }
    let Some(provider) = provider_for(&policy.service, &policy.verification) else {
        return Ok(not_ok(
            "This server has no way to ask the service.".to_owned(),
        ));
    };
    let secret = {
        let mut conn = ctx.app.db.acquire().await?;
        open_secret(&mut conn).await?
    };
    let Some(secret) = secret else {
        return Ok(not_ok("No secret is kept for the service.".to_owned()));
    };
    let config = Config {
        site_key: policy.site_key.trim().to_owned(),
        project_id: policy.project_id.trim().to_owned(),
        secret,
        timeout: policy.timeout,
    };
    let answer = provider
        .verify(&Request {
            token: TEST_TOKEN,
            act: Act::SignIn,
            remote_ip: None,
            user_agent: None,
            config: &config,
        })
        .await;
    Ok(match answer {
        Ok(_) => HumanCheckTestReply {
            ok: true,
            error: None,
        },
        Err(unreachable) => not_ok(match unreachable.why {
            Why::Secret => "The service does not accept the secret. Set it again.".to_owned(),
            Why::Setup => format!(
                "The service does not know the site key or the project, or the answer cannot be used ({}).",
                unreachable.detail
            ),
            Why::Timeout | Why::Connect | Why::Status | Why::Unreadable | Why::Edge => {
                format!("The service could not be reached ({unreachable}).")
            }
        }),
    })
}

/// Refuses a write that would switch the human check on, or move it to another service or address, while the web
/// edge's content policy has no room for that service: no browser could load its widget, nobody could pass, and the
/// forms would be shut. The edge is looked at anew for it, since the likeliest reason to try again is that it was
/// just set. Where the policy cannot be read nothing is refused, and where only some other key is written, or the
/// service stays as it was, nothing is looked at.
pub async fn before_setting(ctx: &Ctx, key: &str, value: &serde_json::Value) -> ApiResult<()> {
    if !matches!(
        key,
        keys::HUMAN_CHECK_PROVIDER | keys::HUMAN_CHECK_SITE_KEY | keys::HUMAN_CHECK_ALTERNATE_HOST
    ) {
        return Ok(());
    }
    let before = {
        let mut conn = ctx.app.db.acquire().await?;
        policy(&mut conn).await?
    };
    let mut after = before.clone();
    match key {
        keys::HUMAN_CHECK_PROVIDER => {
            after.service = match value.as_str() {
                Some("off") | None => String::new(),
                Some(word) => word.to_owned(),
            };
        }
        keys::HUMAN_CHECK_SITE_KEY => {
            after.site_key = value.as_str().unwrap_or_default().to_owned();
        }
        _ => after.alternate_host = value.as_bool().unwrap_or(false),
    }
    let place = |policy: &Policy| {
        (
            policy.service.clone(),
            policy.service == RECAPTCHA && policy.alternate_host,
        )
    };
    let moved = place(&after) != place(&before);
    let switched_on = after.active() && !before.active();
    if after.service.is_empty() || !(moved || switched_on) {
        return Ok(());
    }
    match edge_of(&ctx.app.config.system.public_url, &after, true).await {
        Edge::Blocks { via, lacking } => Err(ApiError::new(
            ErrorCode::HumanCheckEdgeBlocked,
            "the web edge's content policy does not let pages reach the service",
        )
        .with("lacking", lacking)
        .with(
            "via",
            match via {
                Via::Public => "public",
                Via::Stack => "stack",
            },
        )),
        Edge::Allows(_) | Edge::Unknown => Ok(()),
    }
}

/// Switches the human check off, as the system: for when nobody can get past it. Everything it was set to stays, so
/// that the person who switches it on again finds it as it was. Answers whether anything changed.
pub async fn switch_off(
    conn: &mut Conn,
    origin: &orch_db::audit::Origin,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<bool> {
    let written = orch_settings::write::write(
        conn,
        &orch_settings::write::Write {
            actor: Actor::System,
            target: orch_settings::Target::System,
            key: keys::HUMAN_CHECK_PROVIDER,
            value: json!("off"),
            locked: None,
            expected_revision: None,
            may_lock: true,
            origin,
        },
        now,
    )
    .await
    .map_err(|error| {
        ApiError::internal(format!(
            "the human check could not be switched off: {error:?}"
        ))
    })?;
    Ok(written.changed)
}
