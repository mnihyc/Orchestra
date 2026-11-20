//! Renewal of the cookie session shared by the shell and the addressed thread.
use crate::middleware::authentication::{SESSION_PREFIX, new_token, token_hash};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Answer, Ctx};
use crate::sign_in::{NOT_KEPT, access_life, people, session_cookie, session_life};
use http::header::{CACHE_CONTROL, SET_COOKIE};
use orch_api_contract::auth::AuthResult;
use orch_api_contract::common::{ErrorCode, SESSION_COOKIE};
use orch_db::queries::identity;
use orch_types::Time;

/// The value of the session cookie of a request, where it has one in the
/// form of one.
pub fn session_value(ctx: &Ctx) -> Option<&str> {
    ctx.cookie(SESSION_COOKIE)
        .filter(|value| value.starts_with(SESSION_PREFIX))
}

/// Anonymous discovery is a successful empty result; a real cookie uses the
/// same guarded refresh and session lifetime as every ordinary renewal.
pub async fn restore(ctx: Ctx) -> ApiResult<Answer<Option<AuthResult>>> {
    if session_value(&ctx).is_none() {
        return Ok(Answer::from(None).header(CACHE_CONTROL, NOT_KEPT));
    }
    let answer = refresh(ctx).await?;
    Ok(Answer {
        body: Some(answer.body),
        status: answer.status,
        headers: answer.headers,
    })
}

/// Gives a new access token to the browser that holds the session cookie, and
/// renews the session: a session lasts as long after its last use as the
/// system says.
pub async fn refresh(mut ctx: Ctx) -> ApiResult<Answer<AuthResult>> {
    let Some(value) = session_value(&ctx).map(str::to_owned) else {
        return Err(ApiError::new(
            ErrorCode::AuthRequired,
            "there is no session cookie",
        ));
    };
    let invalid = || {
        ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the session has ended or is unknown",
        )
    };
    let auth = &ctx.app.config.auth;
    let mut tx = ctx.app.db.begin().await?;
    let hash = token_hash(&value);
    // Retained cookie proof identifies only this request's prior session. The live
    // session resolver and the guarded renewal below remain the grant check.
    let proof = identity::session_proof_by_refresh_hash(&mut tx, &hash)
        .await?
        .ok_or_else(invalid)?;
    let user = identity::user_by_id(&mut tx, proof.user_id)
        .await?
        .ok_or_else(invalid)?;
    ctx.now = ctx.app.clock.now();
    let session = match identity::session_by_refresh_hash(&mut tx, &hash, ctx.now).await? {
        Some(session) => session,
        None => {
            let mut error = invalid();
            if let Some(proof) = identity::session_proof_by_refresh_hash(&mut tx, &hash).await?
                && let Some(fact) =
                    crate::middleware::authentication::recorded_session_end(&proof, ctx.now)
            {
                error = error.session_ended(fact);
            }
            return Err(error);
        }
    };
    if user.suspended() {
        return Err(
            ApiError::new(ErrorCode::AuthSuspended, "this user is suspended")
                .session_ended(crate::middleware::authentication::unavailable_session()),
        );
    }
    let life = session_life(auth);
    if !identity::renew_session(&mut tx, session.id, ctx.now, ctx.now + life).await? {
        return Err(invalid());
    }
    let access = new_token(crate::middleware::authentication::ACCESS_TOKEN_PREFIX);
    let access_expires_at = ctx.now + access_life(auth);
    identity::add_access_token(&mut tx, &token_hash(&access), session.id, access_expires_at)
        .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let mut conn = ctx.app.db.acquire().await?;
    // Profile projection and last-seen bookkeeping hold no renewal lock.
    identity::touch_last_seen(&mut conn, user.id, ctx.now).await?;
    let me = people::me(&mut conn, &user, ctx.now).await?;
    Ok(Answer::from(AuthResult {
        access_token: access,
        access_expires_at: Time::new(access_expires_at),
        user: me,
    })
    // The same cookie again, so that its life slides with the session's.
    .header(CACHE_CONTROL, NOT_KEPT)
    .header(SET_COOKIE, &session_cookie(&value, life)))
}
