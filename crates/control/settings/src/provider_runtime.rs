//! A provider's chosen harness home and sign-in admission, shared by the API, engine and schedule.

use orch_db::{
    Conn,
    queries::resources::{provider_runtime, thread_engine},
};
use orch_driver_contract::runtime::EnvironmentDataSource;
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};

use crate::{Error, Place, get};

/// Explicit runtime choice over the profile's choice; absence preserves the adapter's declared default.
pub async fn provider_home_choice(
    conn: &mut Conn,
    provider: Id,
) -> Result<Option<EnvironmentDataSource>, Error> {
    if let Some(home) = get(conn, Place::Provider(provider), keys::RUNTIME_HOME).await? {
        return Ok(Some(home));
    }
    let row = provider_runtime::state(conn, provider)
        .await?
        .ok_or(Error::NotFound)?;
    if row.key_machine_id.is_some() && row.key_machine_id != row.machine_id {
        return Ok(Some(EnvironmentDataSource::MadeByOrch));
    }
    profile_home_choice(conn, row.agent_id).await
}

/// Unit storage needs a concrete source; an unconfigured unit gets its existing owned binding.
pub async fn provider_home(conn: &mut Conn, provider: Id) -> Result<EnvironmentDataSource, Error> {
    Ok(provider_home_choice(conn, provider)
        .await?
        .unwrap_or(EnvironmentDataSource::MadeByOrch))
}

/// A distinct native keeper uses its profile's home; a runtime's selected path never redirects another machine's sign-in.
pub async fn credential_home(
    conn: &mut Conn,
    provider: Id,
) -> Result<EnvironmentDataSource, Error> {
    Ok(credential_home_choice(conn, provider)
        .await?
        .unwrap_or(EnvironmentDataSource::MadeByOrch))
}

/// The keeper retains its adapter default when neither it nor its profile chose a home.
pub async fn credential_home_choice(
    conn: &mut Conn,
    provider: Id,
) -> Result<Option<EnvironmentDataSource>, Error> {
    let row = provider_runtime::state(conn, provider)
        .await?
        .ok_or(Error::NotFound)?;
    if row.key_machine_id.is_none() || row.key_machine_id == row.machine_id {
        provider_home_choice(conn, provider).await
    } else {
        profile_home_choice(conn, row.agent_id).await
    }
}

/// Concrete source for unit/data planning; native starts preserve absence through profile_home_choice.
pub async fn profile_home(
    conn: &mut Conn,
    agent: Option<Id>,
) -> Result<EnvironmentDataSource, Error> {
    Ok(profile_home_choice(conn, agent)
        .await?
        .unwrap_or(EnvironmentDataSource::MadeByOrch))
}

/// An unset profile does not redirect a harness away from its own signed-in home.
pub async fn profile_home_choice(
    conn: &mut Conn,
    agent: Option<Id>,
) -> Result<Option<EnvironmentDataSource>, Error> {
    let profile = match agent {
        Some(agent) => thread_engine::agent(conn, agent).await?,
        None => None,
    };
    profile
        .as_ref()
        .map_or(Ok(None), |profile| profile_home_in(&profile.profile))
}

/// The home declared by a profile already read with a page of providers.
pub fn profile_home_in(profile: &Json) -> Result<Option<EnvironmentDataSource>, Error> {
    let Some(home) = profile
        .pointer("/attach/home")
        .filter(|home| !home.is_null())
    else {
        return Ok(None);
    };
    // Existing stored profiles named their borrowed path directly; QU-11's typed choice reads here too.
    if let Some(path) = home.as_str() {
        return Ok(Some(EnvironmentDataSource::ExistingDirectory {
            path: path.to_owned(),
        }));
    }
    serde_json::from_value(home.clone())
        .map(Some)
        .map_err(|error| Error::NotOfThatType {
            key: keys::RUNTIME_HOME.into(),
            reason: error.to_string(),
        })
}

/// Whether a machine's reported facts identify the chosen native home as its own.
#[must_use]
pub fn machine_home_owned(runtime_id: Option<Id>, facts: &Json, kind: &str) -> bool {
    runtime_id.is_some()
        || facts
            .get("adapters")
            .and_then(Json::as_array)
            .is_some_and(|adapters| {
                adapters.iter().any(|adapter| {
                    adapter.get("kind").and_then(Json::as_str) == Some(kind)
                        && adapter.get("default_home_owned").and_then(Json::as_bool) == Some(true)
                })
            })
}

/// Ownership of an unconfigured home is a fact of this adapter, not of the machine's display kind.
pub async fn harness_home_owned(conn: &mut Conn, machine: Id, kind: &str) -> Result<bool, Error> {
    let reported: Option<(bool, orch_types::Json)> =
        sqlx::query_as("SELECT runtime_id IS NOT NULL,facts FROM machines WHERE id=$1")
            .bind(machine)
            .fetch_optional(conn)
            .await
            .map_err(orch_db::DbError::from)?;
    Ok(reported
        .is_some_and(|(unit, facts)| machine_home_owned(unit.then_some(machine), &facts, kind)))
}

/// Why Orch may, or may not, change the native sign-in in this selected home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSignIn {
    /// A home made by Orch, independent of the kind of machine or its mount path.
    pub home_owned: bool,
    /// The credential keeper's physical machine switch, absent until reported.
    pub policy_allowed: Option<bool>,
    /// Native sign-in methods and renewal are permitted in the selected home.
    pub allowed: bool,
}

/// Uses the credential keeper's physical policy; a unit's empty local policy never substitutes for it.
pub async fn provider_sign_in(conn: &mut Conn, provider: Id) -> Result<ProviderSignIn, Error> {
    let home_owned = match credential_home_choice(conn, provider).await? {
        Some(EnvironmentDataSource::MadeByOrch) => true,
        Some(EnvironmentDataSource::ExistingDirectory { .. }) => false,
        None => {
            let row = provider_runtime::state(conn, provider)
                .await?
                .ok_or(Error::NotFound)?;
            match row.key_machine_id.or(row.machine_id) {
                Some(machine) => {
                    harness_home_owned(conn, machine, row.harness_kind.as_deref().unwrap_or(""))
                        .await?
                }
                None => true,
            }
        }
    };
    let policy_allowed = orch_db::queries::environment_facts::credential_policy(conn, provider)
        .await?
        .and_then(|policy| {
            policy
                .get("provider.signin")
                .and_then(orch_types::Json::as_bool)
        });
    Ok(ProviderSignIn {
        home_owned,
        policy_allowed,
        allowed: home_owned || policy_allowed == Some(true),
    })
}
