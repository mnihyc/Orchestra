//! The existing account second-step requirement, independent of HTTP sessions.
//! HTTP keeps its original given-session proof; native captured work uses current setup/hold facts.

use crate::{Place, Snapshot, Wanted};
use orch_db::Conn;
use orch_db::queries::identity::UserRow;
use orch_db::queries::second_step as rows;
use orch_settings_contract::provenance::SourceScope;
use orch_settings_contract::registry::{self, keys};
use orch_types::Id;
use std::collections::HashMap;

/// Why current account setup policy could not be read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A current account or membership query failed.
    #[error(transparent)]
    Db(#[from] orch_db::DbError),
    /// Current settings could not be evaluated.
    #[error(transparent)]
    Settings(#[from] crate::Error),
    /// The built-in account setup setting is unavailable.
    #[error("the setting auth.second_step is not in the registry")]
    MissingDefinition,
}

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

fn is_service(user: &UserRow) -> bool {
    user.kind == "service"
}

/// What `auth.second_step` resolves to at `place`: whether it requires the step, and where that is
/// set (`None` for the system). A place that is gone requires nothing.
async fn resolved_at(conn: &mut Conn, place: Place) -> Result<Option<Option<Id>>, Error> {
    let def = registry::get(keys::AUTH_SECOND_STEP).ok_or(Error::MissingDefinition)?;
    // A sign-in's Me response must not inline the general settings state machine.
    let snapshot = match Box::pin(Snapshot::load(conn, place, Wanted::Keys(&[def]))).await {
        Ok(snapshot) => snapshot,
        Err(crate::Error::NotFound) => return Ok(None),
        Err(error) => return Err(error.into()),
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
pub async fn requirements_of(
    conn: &mut Conn,
    people: &[Id],
) -> Result<HashMap<Id, Requirement>, Error> {
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
pub async fn requirement(conn: &mut Conn, user: &UserRow) -> Result<Requirement, Error> {
    if is_service(user) {
        return Ok(Requirement::default());
    }
    Ok(requirements_of(conn, &[user.id])
        .await?
        .remove(&user.id)
        .unwrap_or_default())
}
