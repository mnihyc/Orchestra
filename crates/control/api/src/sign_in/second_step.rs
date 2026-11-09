//! The second step an org can require: of whom it is required, where a request waits for it, and what the person and their org's managers are told of it.
//!
//! `auth.second_step` is resolved at the root org of each tree a person is a
//! member of, as `auth.email_code` is, their own org left out; for somebody in
//! no team org the system's value holds. Where it says `required`, that root
//! org and everything inside it wait for the person's sign-in to have a
//! second step: while they have none set up, a request there is answered
//! `403 auth.second_step_setup`, with the places that require it and where to
//! set one up; while they have one and this browser session has not given it,
//! `401 auth.second_step_required`, with the proofs that would. Their own org,
//! Security, signing out and everything outside those trees stay open, and
//! nothing on any page changes: the wait is a hold on the request
//! ([`SecondStepHold`]), lifted by the next request after the step is set up
//! or given. Another org that requires nothing does not weaken it.
//!
//! An API token of a person waits only for the setup: a token is no session
//! that could give the step. A service identity has no human step and never
//! waits. An identity provider's sign-in says how the first proof was made
//! and nothing of a second: it satisfies no requirement by itself.

use crate::middleware::authentication::{Authed, Via};
use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use chrono::Duration;
use orch_api_contract::auth::SignInProof;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::directory::MemberSecondStep;
use orch_api_contract::nodes::NodeOrSystem;
use orch_authz::Principal;
use orch_authz::suspension::SecondStepHold;
use orch_db::Conn;
use orch_db::queries::identity::UserRow;
use orch_db::queries::second_step as rows;
use orch_types::Id;
use std::collections::HashMap;

/// Where a person sets a second step up, which a refusal for its setup names.
pub const SECURITY_PATH: &str = "/settings/security";

/// How often one person's use of the things of an org is written as when they were last seen
/// there: the realtime heartbeat, so that somebody at work costs one write in that time and "Last
/// seen" is still right to the minute.
pub const LAST_SEEN_EVERY: Duration =
    Duration::seconds(orch_realtime_contract::version::HEARTBEAT_SECS as i64);

pub use orch_engine::operation_scope::account_policy::{Requirement, configured};

/// The second proofs the person has set up, as a sign-in names them.
pub async fn methods(conn: &mut Conn, user: &UserRow) -> ApiResult<Vec<SignInProof>> {
    let mut methods = Vec::new();
    if user.totp_on() {
        methods.push(SignInProof::Totp);
        if orch_db::queries::recovery_codes::status(conn, user.id)
            .await?
            .remaining
            > 0
        {
            methods.push(SignInProof::RecoveryCode);
        }
    }
    if user.email_code_on() {
        methods.push(SignInProof::EmailCode);
    }
    Ok(methods)
}

fn policy_error(error: orch_engine::EngineError) -> ApiError {
    match error {
        orch_engine::EngineError::Db(error) => error.into(),
        other => ApiError::internal(other),
    }
}

async fn requirements_of(conn: &mut Conn, people: &[Id]) -> ApiResult<HashMap<Id, Requirement>> {
    orch_engine::operation_scope::account_policy::requirements_of(conn, people)
        .await
        .map_err(policy_error)
}

/// Who requires a second step of `user`: nobody of a service identity.
pub async fn requirement(conn: &mut Conn, user: &UserRow) -> ApiResult<Requirement> {
    orch_engine::operation_scope::account_policy::requirement(conn, user)
        .await
        .map_err(policy_error)
}

/// The places a requirement is set at, as a person reads them: the orgs by name, and the system.
pub async fn places(conn: &mut Conn, sources: &[Option<Id>]) -> ApiResult<Vec<NodeOrSystem>> {
    let mut named = Vec::with_capacity(sources.len());
    for source in sources {
        named.push(match source {
            None => NodeOrSystem::SYSTEM,
            Some(org) => NodeOrSystem::Node(refusals::named_node(conn, *org).await?),
        });
    }
    Ok(named)
}

/// The caller's principal, held out of the team orgs that wait for their second step, if any do:
/// while they have none set up, or, in a browser session, while this session has not given the one
/// they have. The quick answers come first: a service identity, and a session that gave its step,
/// read nothing more; where no place requires the step at all, one read says so.
pub async fn hold(conn: &mut Conn, authed: &Authed, principal: Principal) -> ApiResult<Principal> {
    let session = match authed.via {
        Via::Session { session_id } => Some(session_id),
        Via::Token { .. } | Via::OwnerRead { .. } => None,
    };
    hold_for(conn, &authed.user, session, principal).await
}

/// A restored actual key follows the existing token rule, independent of the confirming browser.
pub(crate) async fn hold_key_origin(
    conn: &mut Conn,
    user: &UserRow,
    principal: Principal,
) -> ApiResult<Principal> {
    if principal.user() != user.id || principal.caller().grant().is_none() {
        return Err(ApiError::internal(
            "a saved key origin changed its account or credential kind",
        ));
    }
    hold_for(conn, user, None, principal).await
}

async fn hold_for(
    conn: &mut Conn,
    user: &UserRow,
    session: Option<Id>,
    principal: Principal,
) -> ApiResult<Principal> {
    let Some(mut hold) =
        orch_engine::operation_scope::account_policy::session_hold(conn, user, session)
            .await
            .map_err(policy_error)?
    else {
        return Ok(principal);
    };
    hold.methods = methods(conn, user)
        .await?
        .iter()
        .filter_map(|method| serde_json::to_value(method).ok())
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    Ok(principal.held_for_second_step(hold))
}

/// The refusal of a request that waits for a second step: `403 auth.second_step_setup` with the
/// places that require it and where to set one up, or `401 auth.second_step_required` with the
/// proofs that this session can give.
pub async fn refusal(conn: &mut Conn, hold: &SecondStepHold) -> ApiError {
    let scopes = match places(conn, &hold.sources).await {
        Ok(scopes) => scopes,
        Err(error) => return error,
    };
    if hold.setup {
        ApiError::new(
            ErrorCode::AuthSecondStepSetup,
            "an org of yours requires a second step: set one up first",
        )
        .with("scopes", scopes)
        .with("path", SECURITY_PATH)
    } else {
        ApiError::new(
            ErrorCode::AuthSecondStepRequired,
            "give your second step in this session first",
        )
        .with("methods", &hold.methods)
        .with("scopes", scopes)
    }
}

/// How each of the people `members` stands with a second step, as the managers of an org read it:
/// whether one is set up, or their first proof is an identity provider's, and whether an org of
/// theirs requires one. A service identity has no human step and is left out.
pub async fn of_members(
    conn: &mut Conn,
    members: &[Id],
    local_login: bool,
) -> ApiResult<HashMap<Id, (MemberSecondStep, bool)>> {
    let facts: Vec<rows::SignInFacts> = rows::sign_in_facts(conn, members)
        .await?
        .into_iter()
        .filter(|person| !person.service)
        .collect();
    let ids: Vec<Id> = facts.iter().map(|person| person.id).collect();
    let required = requirements_of(conn, &ids).await?;
    Ok(facts
        .into_iter()
        .map(|person| {
            let state = if person.totp_on || person.email_code_on {
                MemberSecondStep::Enabled
            } else if !person.has_password || (!local_login && !person.is_system_admin) {
                // Signing in by a password is not theirs: their first proof is an identity provider's.
                MemberSecondStep::Sso
            } else {
                MemberSecondStep::Missing
            };
            let requires = required.get(&person.id).is_some_and(Requirement::required);
            (person.id, (state, requires))
        })
        .collect())
}

/// What `people::me` adds of the second step: whether one is required, and where.
pub async fn of_me(conn: &mut Conn, user: &UserRow) -> ApiResult<(bool, Vec<NodeOrSystem>)> {
    let requirement = requirement(conn, user).await?;
    let sources = places(conn, &requirement.sources).await?;
    Ok((requirement.required(), sources))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_requirement_counts_where_it_is_set_and_not_only_what_waits_for_it() {
        let nobody = Requirement::default();
        assert!(!nobody.required());
        let by_the_system_alone = Requirement {
            orgs: Vec::new(),
            sources: vec![None],
        };
        assert!(
            by_the_system_alone.required(),
            "somebody in no team org is held to the system's value, though no org waits"
        );
    }

    #[test]
    fn last_seen_is_written_at_most_once_a_heartbeat() {
        assert_eq!(LAST_SEEN_EVERY, Duration::seconds(25));
    }
}
