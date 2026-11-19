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
use orch_settings::{Place, Snapshot, Wanted};
use orch_settings_contract::provenance::SourceScope;
use orch_settings_contract::registry::{self, keys};
use orch_types::Id;
use std::collections::HashMap;

/// Where a person sets a second step up, which a refusal for its setup names.
pub const SECURITY_PATH: &str = "/settings/security";

/// How often one person's use of the things of an org is written as when they were last seen
/// there: the realtime heartbeat, so that somebody at work costs one write in that time and "Last
/// seen" is still right to the minute.
pub const LAST_SEEN_EVERY: Duration =
    Duration::seconds(orch_realtime_contract::version::HEARTBEAT_SECS as i64);

/// The value of `auth.second_step` that requires a second step.
const REQUIRED: &str = "required";

/// Who requires a second step of a person.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Requirement {
    /// The root orgs of their team trees that require it: what waits for it.
    pub orgs: Vec<Id>,
    /// Where it is set, each once: an org, or `None` for the system.
    pub sources: Vec<Option<Id>>,
}

impl Requirement {
    /// Whether a second step is required of them at all: by an org of theirs, or by the system for
    /// somebody in no team org.
    #[must_use]
    pub fn required(&self) -> bool {
        !self.sources.is_empty()
    }
}

/// Whether the person has a second step set up: an authenticator, or a mailed code they switched on
/// (which is switched off for them when an org of theirs stops allowing it).
#[must_use]
pub fn configured(user: &UserRow) -> bool {
    user.totp_on() || user.email_code_on()
}

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

/// Whether `user` is a service identity, which has no human second step.
fn is_service(user: &UserRow) -> bool {
    user.kind == "service"
}

/// What `auth.second_step` resolves to at `place`: whether it requires the step, and where that is
/// set (`None` for the system). A place that is gone requires nothing.
async fn resolved_at(conn: &mut Conn, place: Place) -> ApiResult<Option<Option<Id>>> {
    let def = registry::get(keys::AUTH_SECOND_STEP)
        .ok_or_else(|| ApiError::internal("the setting auth.second_step is not in the registry"))?;
    // A sign-in's Me response must not inline the general settings state machine.
    let snapshot = match Box::pin(Snapshot::load(conn, place, Wanted::Keys(&[def]))).await {
        Ok(snapshot) => snapshot,
        Err(orch_settings::Error::NotFound) => return Ok(None),
        Err(error) => return Err(ApiError::internal(error)),
    };
    let resolved = snapshot.resolve(def);
    if resolved.value.as_ref().and_then(|value| value.as_str()) != Some(REQUIRED) {
        return Ok(None);
    }
    Ok(Some(match resolved.source.scope {
        SourceScope::System | SourceScope::Default => None,
        _ => resolved.source.node,
    }))
}

/// Who requires a second step of each of `people`, read together: each distinct root org once.
/// A person in no team org is held to the system's value, which waits nowhere.
async fn requirements_of(conn: &mut Conn, people: &[Id]) -> ApiResult<HashMap<Id, Requirement>> {
    let mut found: HashMap<Id, Requirement> = people
        .iter()
        .map(|person| (*person, Requirement::default()))
        .collect();
    if people.is_empty() || !rows::required_anywhere(conn).await? {
        return Ok(found);
    }
    let trees = rows::root_orgs_of(conn, people).await?;
    let mut roots: Vec<Id> = trees.iter().map(|(_, org)| *org).collect();
    roots.sort_unstable();
    roots.dedup();
    let mut at_root: HashMap<Id, Option<Option<Id>>> = HashMap::new();
    for root in roots {
        let resolved = resolved_at(conn, Place::Node(root)).await?;
        at_root.insert(root, resolved);
    }
    let mut system: Option<Option<Option<Id>>> = None;
    for person in people {
        let theirs: Vec<Id> = trees
            .iter()
            .filter(|(user, _)| user == person)
            .map(|(_, org)| *org)
            .collect();
        let requirement = found.entry(*person).or_default();
        if theirs.is_empty() {
            let value = match system {
                Some(value) => value,
                None => {
                    let value = resolved_at(conn, Place::System).await?;
                    system = Some(value);
                    value
                }
            };
            if let Some(source) = value {
                requirement.sources.push(source);
            }
            continue;
        }
        for root in theirs {
            if let Some(Some(source)) = at_root.get(&root) {
                requirement.orgs.push(root);
                if !requirement.sources.contains(source) {
                    requirement.sources.push(*source);
                }
            }
        }
    }
    Ok(found)
}

/// Who requires a second step of `user`: nobody of a service identity.
pub async fn requirement(conn: &mut Conn, user: &UserRow) -> ApiResult<Requirement> {
    if is_service(user) {
        return Ok(Requirement::default());
    }
    Ok(requirements_of(conn, &[user.id])
        .await?
        .remove(&user.id)
        .unwrap_or_default())
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
    let user = &authed.user;
    if is_service(user) {
        return Ok(principal);
    }
    let set_up = configured(user);
    if set_up {
        match authed.via {
            // A token is no session that could give the step: it waits only for its setup.
            Via::Token { .. } => return Ok(principal),
            Via::Session { session_id } => {
                if rows::session_given_at(conn, session_id).await?.is_some() {
                    return Ok(principal);
                }
            }
        }
    }
    let requirement = requirement(conn, user).await?;
    if requirement.orgs.is_empty() {
        return Ok(principal);
    }
    Ok(principal.held_for_second_step(SecondStepHold {
        orgs: requirement.orgs,
        setup: !set_up,
        sources: requirement.sources,
        methods: methods(conn, user)
            .await?
            .iter()
            .filter_map(|method| serde_json::to_value(method).ok())
            .filter_map(|value| value.as_str().map(str::to_owned))
            .collect(),
    }))
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
