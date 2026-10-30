//! What a unit's request tells its host's operator hooks about what the unit serves, and why the unit was made.
//!
//! A host's node runs its operator's lifecycle hooks around the start and the stop of every unit, and tells them what
//! the unit serves: for an environment its identifier, its name, the names of the places down to its home, its
//! template and its generation; for a provider's runtime the provider's identifier and name. It also tells why the
//! unit was made: for a new environment or runtime (`start`), as the successor of another on the same volumes
//! (`recreate`), or on the host an environment moves to (`move`). The engine puts this into the request when it
//! makes a unit (`RuntimeSpec.subject`). Nothing of it is a secret or anything of what is worked on; the names are as
//! they are when the unit is made.

use orch_db::{Conn, DbError};
use orch_driver_contract::runtime::{SubjectEnvironment, SubjectProvider, UnitReason, UnitSubject};
use orch_types::{Id, Json};

/// What joins the names of the places on a path.
const PATH_SEPARATOR: &str = " / ";

/// Why a unit made by a job of the kind `job` is made.
#[must_use]
pub fn made_for(job: &str) -> UnitReason {
    use orch_db::queries::jobs::kind;
    match job {
        kind::ENVIRONMENT_RECREATE | kind::PROVIDER_RECREATE => UnitReason::Recreate,
        kind::ENVIRONMENT_MOVE => UnitReason::Move,
        _ => UnitReason::Start,
    }
}

/// The names of the places from the root org down to `node`, joined by ` / `; empty for a node that is not there.
pub async fn path_names(conn: &mut Conn, node: Id) -> Result<String, DbError> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT place.name FROM nodes AS here \
         CROSS JOIN LATERAL unnest(here.path) WITH ORDINALITY AS step(id, ord) \
         JOIN nodes AS place ON place.id = step.id \
         WHERE here.id = $1 ORDER BY step.ord",
    )
    .bind(node)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    Ok(names.join(PATH_SEPARATOR))
}

/// What a unit of an environment serves.
#[derive(Debug, Clone, Copy)]
pub struct OfEnvironment<'a> {
    /// The kind of the job that makes the unit.
    pub job: &'a str,
    /// The environment.
    pub id: Id,
    /// Its name.
    pub name: &'a str,
    /// Its home.
    pub home: Id,
    /// The template it is made from.
    pub template_id: Option<Id>,
    /// That template's name.
    pub template: &'a str,
    /// Its generation.
    pub generation: i64,
}

/// The subject of an environment's unit, as the request carries it.
pub async fn environment(conn: &mut Conn, of: OfEnvironment<'_>) -> Result<Json, DbError> {
    let subject = UnitSubject {
        made_for: made_for(of.job),
        environment: Some(SubjectEnvironment {
            id: of.id,
            name: of.name.to_owned(),
            path: path_names(conn, of.home).await?,
            template_id: of.template_id,
            template: of.template.to_owned(),
            generation: of.generation,
        }),
        provider: None,
    };
    Ok(serde_json::to_value(subject).unwrap_or(Json::Null))
}

/// The subject of a provider's runtime, as the request carries it.
#[must_use]
pub fn provider(job: &str, id: Id, name: &str) -> Json {
    let subject = UnitSubject {
        made_for: made_for(job),
        environment: None,
        provider: Some(SubjectProvider {
            id,
            name: name.to_owned(),
        }),
    };
    serde_json::to_value(subject).unwrap_or(Json::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unit_is_made_for_a_start_a_recreate_or_a_move_as_its_job_says() {
        assert_eq!(made_for("environment_provision"), UnitReason::Start);
        assert_eq!(made_for("environment_recreate"), UnitReason::Recreate);
        assert_eq!(made_for("provider_recreate"), UnitReason::Recreate);
        assert_eq!(made_for("environment_move"), UnitReason::Move);
        assert_eq!(made_for("provider_provision"), UnitReason::Start);
    }

    #[test]
    fn a_providers_subject_names_the_provider_and_no_environment() {
        let subject = provider("provider_provision", orch_types::new_id(), "OpenAI team");
        assert_eq!(subject["made_for"], "start");
        assert_eq!(subject["provider"]["name"], "OpenAI team");
        assert!(subject.get("environment").is_none());
        let read: UnitSubject = serde_json::from_value(subject).unwrap();
        assert!(read.environment.is_none());
    }
}
