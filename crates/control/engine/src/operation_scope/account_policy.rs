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
    orch_authz::http_origin::session_hold(conn, user, session)
        .await
        .map_err(error)
}
