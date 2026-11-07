//! The existing account setup policy, shared with native delivery workers.

use crate::EngineError;
use orch_db::Conn;
use orch_db::queries::identity::UserRow;
use orch_types::Id;
use std::collections::HashMap;

pub use orch_settings::account_policy::{Requirement, configured};

fn error(error: orch_settings::account_policy::Error) -> EngineError {
    match error {
        orch_settings::account_policy::Error::Db(error) => error.into(),
        orch_settings::account_policy::Error::Settings(error) => error.into(),
        orch_settings::account_policy::Error::MissingDefinition => {
            EngineError::unreadable("the setting auth.second_step is not in the registry")
        }
    }
}

/// Who requires a second step of each person, using the existing current settings reader.
pub async fn requirements_of(
    conn: &mut Conn,
    people: &[Id],
) -> Result<HashMap<Id, Requirement>, EngineError> {
    orch_settings::account_policy::requirements_of(conn, people)
        .await
        .map_err(error)
}

/// Who currently requires account setup from this native actor.
pub async fn requirement(conn: &mut Conn, user: &UserRow) -> Result<Requirement, EngineError> {
    orch_settings::account_policy::requirement(conn, user)
        .await
        .map_err(error)
}

/// The existing account/session hold, without a transport's presentation of proof methods.
/// An API key supplies no browser session; a real browser must retain its actual proof.
pub async fn session_hold(
    conn: &mut Conn,
    user: &UserRow,
    session: Option<Id>,
) -> Result<Option<orch_authz::suspension::SecondStepHold>, EngineError> {
    if user.kind == "service" {
        return Ok(None);
    }
    let set_up = configured(user);
    if set_up {
        match session {
            None => return Ok(None),
            Some(session)
                if orch_db::queries::second_step::session_given_at(conn, session)
                    .await?
                    .is_some() =>
            {
                return Ok(None);
            }
            Some(_) => {}
        }
    }
    let requirement = requirement(conn, user).await?;
    if requirement.orgs.is_empty() {
        return Ok(None);
    }
    Ok(Some(orch_authz::suspension::SecondStepHold {
        orgs: requirement.orgs,
        setup: !set_up,
        sources: requirement.sources,
        methods: Vec::new(),
    }))
}
