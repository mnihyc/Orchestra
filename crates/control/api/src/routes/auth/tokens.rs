//! API tokens made narrow and with a fresh proof, within the longest life their places admit, asked again by their key without their secret; and an org's list of the tokens that reach it, each of which it can refuse there.
//!
//! A token made without scopes may use nothing: the page's presets send an
//! explicit list, and everything its person may only when `*` is chosen. The
//! request that makes it carries the maker's fresh proof ([`fresh`]). Its whole
//! life, from now to its end, is checked against `tokens.max_days` where it is
//! made: the system's for a personal token, the orgs down to its node or to the
//! service identity's org as well; an org it reaches later is checked when it
//! is used (`authz` keeps it out). A creation asked again with the same
//! `Idempotency-Key` answers what was made, with no secret: none is kept to be
//! given twice. An org's members managers list the tokens that can reach it and
//! refuse one there, which keeps it out of the org and everything inside from
//! the next request on, and changes nothing elsewhere.
//!
//! [`fresh`]: crate::sign_in::fresh

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::sign_in::record;
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::Scopes;
use orch_api_contract::auth::{
    ApiTokenCreateBody, ApiTokenInfo, OrgToken, OrgTokenList, OrgTokenListQuery, OrgTokenRefuse,
    TokenCreated,
};
use orch_api_contract::common::{
    DEFAULT_LIMIT, ErrorCode, FieldFault, IDEMPOTENCY_HEADER, MAX_LIMIT, Page, Problem,
};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_db::Conn;
use orch_db::audit::Actor;
use orch_db::idempotency::{self, Replay};
use orch_db::queries::tokens;
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};
use serde_json::json;

/// How many characters a token's name has.
const NAME_CHARS: std::ops::RangeInclusive<usize> = 1..=80;
/// How long a creation's key is answered again: a day, as every creating request's key.
const KEY_KEPT_HOURS: i64 = 24;

/// Mounts the org's list of tokens and the refusal.
pub fn mount(registry: &mut Registry) {
    registry
        .on(OrgTokenList, org_tokens)
        .on(OrgTokenRefuse, refuse);
}

/// What a request to make a token asks for, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    /// Its name.
    pub name: String,
    /// Its scope words: none where none were given.
    pub scopes: Vec<String>,
    /// When it ends.
    pub expires_at: Option<DateTime<Utc>>,
}

/// Checks the body of a request to make a token, all of it at once: a name, scopes of the catalogue
/// (left out, none), an end in the future.
pub fn asked(body: &ApiTokenCreateBody, now: DateTime<Utc>) -> ApiResult<Asked> {
    let mut faults = Vec::new();
    let mut fault = |path: &str, message: &str| {
        faults.push(FieldFault {
            path: path.to_owned(),
            message: message.to_owned(),
        });
    };
    let name = body.name.trim().to_owned();
    if !NAME_CHARS.contains(&name.chars().count()) {
        fault("name", "must be 1 to 80 characters");
    }
    let scopes = match Scopes::parse(body.scopes.as_deref().unwrap_or_default()) {
        Ok(parsed) => parsed.to_strings(),
        Err(error) => {
            fault("scopes", &error.to_string());
            Vec::new()
        }
    };
    let expires_at = body.expires_at.map(Time::get);
    if expires_at.is_some_and(|at| at <= now) {
        fault("expires_at", "must be in the future");
    }
    if faults.is_empty() {
        Ok(Asked {
            name,
            scopes,
            expires_at,
        })
    } else {
        Err(Problem::invalid(faults).into())
    }
}

/// Refuses a token whose end is no longer in the future at `now`, the action's own time (an end that
/// passed while the action waited for its turn is refused as it would have been at once), and one whose
/// whole life, from `now` to `expires_at` (none: it never ends), is longer than `tokens.max_days` at
/// `place`: `403 token.lifetime_exceeded` with the bound and the place it comes from.
pub async fn within_lifetime(
    conn: &mut Conn,
    place: Place,
    now: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
) -> ApiResult<()> {
    if expires_at.is_some_and(|end| end <= now) {
        return Err(Problem::invalid(vec![FieldFault {
            path: "expires_at".to_owned(),
            message: "must be in the future".to_owned(),
        }])
        .into());
    }
    let view = orch_settings::resolve(conn, place, keys::TOKENS_MAX_DAYS)
        .await
        .map_err(ApiError::internal)?;
    let Some(max_days) = view
        .resolved
        .value
        .as_ref()
        .and_then(serde_json::Value::as_i64)
    else {
        return Ok(());
    };
    let longest = chrono::Duration::days(max_days);
    if expires_at.is_some_and(|end| end - now <= longest) {
        return Ok(());
    }
    Err(ApiError::new(
        ErrorCode::TokenLifetimeExceeded,
        format!("a token made here may live {max_days} days at most"),
    )
    .with("max_days", max_days)
    .with("scope", view.resolved.source.node))
}

/// The key the request carries, where it carries one.
fn key_of(ctx: &Ctx) -> Option<String> {
    ctx.headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

/// What was answered to the same creation asked before with this request's key, without its secret:
/// looked at before any proof is spent. The same key on another request is a mistake of the client.
pub async fn replayed(ctx: &Ctx, conn: &mut Conn, maker: Id) -> ApiResult<Option<TokenCreated>> {
    let Some(key) = key_of(ctx) else {
        return Ok(None);
    };
    let since = ctx.now - chrono::Duration::hours(KEY_KEPT_HOURS);
    let Some(replay) = idempotency::find(conn, maker, &key, since).await? else {
        return Ok(None);
    };
    if replay.method != "POST" || replay.path != ctx.route.path {
        return Err(ApiError::new(
            ErrorCode::ConflictIdempotencyMismatch,
            "the key was used for another request",
        ));
    }
    let info: ApiTokenInfo = serde_json::from_value(replay.response).map_err(ApiError::internal)?;
    Ok(Some(TokenCreated {
        token: None,
        info,
        already_created: true,
    }))
}

/// Keeps, under the request's key, what a repeat of this creation is answered: the token as it is
/// listed, never its secret. In the transaction that made it.
pub async fn keep(ctx: &Ctx, conn: &mut Conn, maker: Id, info: &ApiTokenInfo) -> ApiResult<()> {
    let Some(key) = key_of(ctx) else {
        return Ok(());
    };
    let replay = Replay {
        method: "POST".to_owned(),
        path: ctx.route.path.to_owned(),
        status: 200,
        response: serde_json::to_value(info).map_err(ApiError::internal)?,
    };
    idempotency::keep(conn, maker, &key, &replay, ctx.now).await?;
    Ok(())
}

/// Lists the tokens that can reach the org, with whether each is refused here and whether its whole
/// life is within what the org admits. No secret, of course.
async fn org_tokens(ctx: Ctx, query: OrgTokenListQuery, (): ()) -> ApiResult<Page<OrgToken>> {
    let org = ctx.id("id")?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let after = match query.cursor.as_deref() {
        None => None,
        Some(text) => Some(
            text.parse::<Id>()
                .map_err(|_| ApiError::invalid("cursor", "is not a cursor of this list"))?,
        ),
    };
    let mut conn = ctx.app.db.acquire().await?;
    let mut rows = tokens::reaching(&mut conn, org, ctx.now, after, i64::from(limit) + 1).await?;
    let more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let bound = orch_settings::get::<i64>(&mut conn, Place::Node(org), keys::TOKENS_MAX_DAYS)
        .await
        .map_err(ApiError::internal)?;
    let users: Vec<Id> = rows.iter().map(|row| row.user_id).collect();
    let refs = authorization::refusals::user_refs(&mut conn, &users).await?;
    let next_cursor = if more {
        rows.last().map(|row| row.id.to_string())
    } else {
        None
    };
    let items = rows
        .into_iter()
        .filter_map(|row| {
            let user = refs.get(&row.user_id)?.clone();
            let lifetime_allowed = bound.is_none_or(|days| {
                row.expires_at
                    .is_some_and(|end| end - row.created_at <= chrono::Duration::days(days))
            });
            let node = row.node_id.map(|id| NodeRef {
                thread_display: None,
                id,
                kind: row
                    .node_kind
                    .as_deref()
                    .and_then(|kind| serde_json::from_value::<NodeKind>(json!(kind)).ok())
                    .unwrap_or(NodeKind::Org),
                name: row.node_name.clone(),
                redacted: false,
            });
            Some(OrgToken {
                info: ApiTokenInfo {
                    id: row.id,
                    name: row.name,
                    scopes: row.scopes,
                    node,
                    created_at: Time::new(row.created_at),
                    last_used_at: row.last_used_at.map(Time::new),
                    expires_at: row.expires_at.map(Time::new),
                },
                user,
                service: row.service,
                refused_here: row.refused_here,
                lifetime_allowed,
            })
        })
        .collect();
    Ok(Page { items, next_cursor })
}

/// Refuses a token in the org and everything inside, once: the token keeps working elsewhere, and
/// nothing it did before is undone. What was decided under its reach is decided again.
async fn refuse(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let org = ctx.id("id")?;
    let token = ctx.id("token_id")?;
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let reaching = tokens::reaching(&mut tx, org, ctx.now, None, i64::MAX).await?;
    if !reaching.iter().any(|row| row.id == token) {
        return Err(ApiError::not_found());
    }
    if tokens::refuse(&mut tx, token, org, principal.user(), ctx.now).await? {
        orch_db::queries::access::bump_epoch(&mut tx).await?;
        record(
            &ctx,
            &mut tx,
            Actor::User(principal.user()),
            orch_db::audit::action!(AuditAction::AuthTokenRefused),
            Some(token),
            "a token was refused in this org",
            json!({ "token": token, "org": org }),
        )
        .await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
