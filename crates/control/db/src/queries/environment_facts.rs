//! Environment services and the policy of their hosting machine, resolved without placement assumptions.

use crate::{Conn, DbError};
use orch_types::{Id, Json};

/// The policy member of one machine report; null alone means no policy was reported.
/// Call `machine`/`of` first when the supplied identity may belong to a unit connector.
#[must_use]
pub fn policy_in(facts: &Json) -> Option<&Json> {
    facts.get("policy").filter(|value| !value.is_null())
}

/// Facts of the machine that actually hosts an environment, distinct from its unit's connector.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct EnvironmentMachine {
    /// The physical machine, when one has been selected.
    pub machine_id: Option<Id>,
    /// The current driver's declaration, or the connector's directory declaration.
    pub descriptor: Option<Json>,
    /// The machine's own ceiling, as it last reported it.
    pub policy: Option<Json>,
}

impl EnvironmentMachine {
    /// Unreported confinement never counts as isolation.
    #[must_use]
    pub fn confinement(&self) -> &str {
        self.descriptor
            .as_ref()
            .and_then(|d| d.get("confinement"))
            .and_then(Json::as_str)
            .unwrap_or("none")
    }

    /// Whether the hosting program attaches directories rather than making units.
    #[must_use]
    pub fn attaches(&self) -> bool {
        self.descriptor
            .as_ref()
            .and_then(|d| d.get("lifecycle"))
            .and_then(Json::as_str)
            == Some("attach")
    }

    /// Whether the declared driver creates a replaceable unit.
    #[must_use]
    pub fn makes_units(&self) -> bool {
        self.descriptor
            .as_ref()
            .and_then(|d| d.get("lifecycle"))
            .and_then(Json::as_str)
            == Some("unit")
    }
}

/// Only handing over the whole environment section selects Orch's shipped permission column.
/// Handing over one environment key still leaves that section under its machine's control.
#[must_use]
pub fn environment_delegated(policy: Option<&Json>) -> bool {
    policy
        .and_then(|policy| policy.get("policy.delegated"))
        .and_then(Json::as_array)
        .is_some_and(|keys| {
            keys.iter()
                .any(|key| matches!(key.as_str(), Some("*" | "environment.*")))
        })
}

/// Permission columns for a frame's environments, from one batch of their physical machine facts.
/// Actual placement is retained elsewhere: only the defaults profile changes for a handed-over directory.
pub async fn access_profiles(
    conn: &mut Conn,
    environments: &[Id],
) -> Result<Vec<(Id, orch_access_contract::permissions::Placement)>, DbError> {
    use orch_access_contract::permissions::Placement;
    if environments.is_empty() {
        return Ok(Vec::new());
    }
    let units: Vec<(Id, bool)> = sqlx::query_as(
        "SELECT id, placement='managed' OR template_id IS NOT NULL FROM environments WHERE id=ANY($1)",
    ).bind(environments).fetch_all(&mut *conn).await?;
    let directories: Vec<Id> = units
        .iter()
        .filter_map(|(id, unit)| (!*unit).then_some(*id))
        .collect();
    let facts: std::collections::HashMap<_, _> = if directories.is_empty() {
        std::collections::HashMap::new()
    } else {
        of_many(conn, &directories).await?.into_iter().collect()
    };
    Ok(units
        .into_iter()
        .map(|(id, unit)| {
            let reported = facts.get(&id);
            let controlled = unit
                || reported.is_some_and(|facts| {
                    facts
                        .descriptor
                        .as_ref()
                        .and_then(|descriptor| descriptor.get("lifecycle"))
                        .and_then(Json::as_str)
                        == Some("unit")
                        || environment_delegated(facts.policy.as_ref())
                });
            (
                id,
                if controlled {
                    Placement::Managed
                } else {
                    Placement::Connected
                },
            )
        })
        .collect())
}

/// Current environment services; a unit connector never substitutes its policy for its host's.
pub async fn of(conn: &mut Conn, environment: Id) -> Result<Option<EnvironmentMachine>, DbError> {
    Ok(of_many(conn, &[environment])
        .await?
        .pop()
        .map(|(_, machine)| machine))
}

/// The same facts for a page of environments in one statement. A permission frame reads it for
/// the environments of its threads: the offered host is looked for only where it is used
/// (inside the lateral, a one-time filter, instead of a join filter applied after reading every
/// host's driver declarations), and a planned host is found through `jobs_environment_host_idx`.
pub async fn of_many(
    conn: &mut Conn,
    environments: &[Id],
) -> Result<Vec<(Id, EnvironmentMachine)>, DbError> {
    type ReportRow = (Id, Option<Id>, Option<Json>, Option<Json>, Option<String>);
    let rows:Vec<ReportRow> = sqlx::query_as(
        "SELECT e.id,CASE WHEN r.id IS NOT NULL OR e.template_id IS NOT NULL THEN h.machine_id ELSE e.machine_id END AS machine_id,
                CASE WHEN r.id IS NOT NULL THEN jsonb_build_object('lifecycle','unit') || COALESCE(h.drivers -> r.driver,'{}'::jsonb)
                     WHEN e.template_id IS NULL THEN m.facts -> 'environment_descriptor'
                     ELSE jsonb_build_object('lifecycle','unit') ||
                         CASE WHEN h.id IS NOT NULL THEN h.drivers -> t.driver ELSE offered.descriptor END END AS descriptor,
                m.facts -> 'policy' AS policy,
                COALESCE((SELECT c.body #>> '{spec,runtime}' FROM commands c WHERE r.id IS NOT NULL AND c.kind = 'node.runtime.create'
                           AND c.body ->> 'runtime_id' = r.id::text AND c.state IN ('queued','sent','accepted','succeeded')
                         ORDER BY c.created_at DESC, c.id DESC LIMIT 1),
                         e.config ->> 'env.runtime', t.spec ->> 'runtime') AS runtime
           FROM environments e
           LEFT JOIN env_templates t ON t.id=e.template_id
           LEFT JOIN LATERAL (SELECT r.id,r.host_id,r.driver FROM runtimes r
               WHERE r.environment_id=e.id AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL
                 AND NOT (e.status='moving' AND e.machine_id IS NOT NULL AND r.generation>e.generation)
               ORDER BY r.created_at DESC LIMIT 1) r ON true
           LEFT JOIN nodes home ON home.id=e.home_node_id
           LEFT JOIN LATERAL (SELECT s.value #>> '{}' AS host_id,s.locked FROM settings s
               WHERE s.key='env.host_id' AND (s.node_id IS NULL OR s.node_id=ANY(home.path))
               ORDER BY s.locked DESC,
                   CASE WHEN s.locked THEN COALESCE(array_position(home.path,s.node_id),0) END ASC,
                   COALESCE(array_position(home.path,s.node_id),0) DESC LIMIT 1) scoped ON true
           LEFT JOIN LATERAL (SELECT j.plan->>'host' AS host_id FROM jobs j
               WHERE j.environment_id=e.id AND j.plan ? 'host' ORDER BY j.created_at DESC LIMIT 1) planned ON true
           LEFT JOIN LATERAL (SELECT target.* FROM hosts target
               WHERE CASE WHEN r.id IS NOT NULL THEN target.id=r.host_id ELSE
                   target.id::text=COALESCE(planned.host_id,CASE WHEN scoped.locked THEN scoped.host_id
                        WHEN e.config ? 'env.host_id' THEN e.config->>'env.host_id' ELSE scoped.host_id END) END
               ORDER BY target.schedulable DESC,target.id LIMIT 1) h ON true
           LEFT JOIN LATERAL (SELECT target.drivers -> t.driver AS descriptor FROM hosts target
               WHERE r.id IS NULL AND e.template_id IS NOT NULL AND h.id IS NULL
                 AND target.drivers ? t.driver AND NOT target.disabled
               ORDER BY target.schedulable DESC,target.id LIMIT 1) offered ON true
           LEFT JOIN machines m ON m.id=CASE WHEN r.id IS NOT NULL OR e.template_id IS NOT NULL THEN h.machine_id ELSE e.machine_id END
          WHERE e.id=ANY($1)",
    ).bind(environments).fetch_all(conn).await?;
    Ok(rows
        .into_iter()
        .map(|(id, machine_id, descriptor, policy, runtime)| {
            (
                id,
                EnvironmentMachine {
                    machine_id,
                    // What the driver declares, laid over by the runtime the unit is made under (or will be): what
                    // holds for this environment, not for the driver as a whole.
                    descriptor: descriptor.map(|declared| {
                        orch_driver_contract::isolation::effective_json(
                            &declared,
                            runtime.as_deref(),
                        )
                    }),
                    policy,
                },
            )
        })
        .collect())
}

/// One provider runtime's machine facts in a batched resource read.
pub async fn hosted_resources(conn: &mut Conn, machine: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT e.id FROM environments e
        LEFT JOIN LATERAL (SELECT r.host_id FROM runtimes r WHERE r.environment_id=e.id AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL ORDER BY r.created_at DESC LIMIT 1) r ON true
        LEFT JOIN hosts h ON h.id=r.host_id WHERE COALESCE(h.machine_id,e.machine_id)=$1 AND e.deleted_at IS NULL
        UNION SELECT p.id FROM providers p
        LEFT JOIN LATERAL (SELECT r.host_id FROM runtimes r WHERE r.provider_id=p.id AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL ORDER BY r.created_at DESC LIMIT 1) r ON true
        LEFT JOIN hosts h ON h.id=r.host_id WHERE COALESCE(h.machine_id,p.machine_id)=$1 AND p.deleted_at IS NULL")
        .bind(machine).fetch_all(conn).await?)
}

/// One provider runtime's machine facts in a batched resource read.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProviderMachine {
    /// The provider whose runtime these facts describe.
    pub provider_id: Id,
    /// Its physical machine's declaration and policy, using the environment resolver's same shape.
    #[sqlx(flatten)]
    pub machine: EnvironmentMachine,
}

/// Current provider runtime services, resolved in one read for a list and without exposing a unit's policy as its host's.
pub async fn providers(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ProviderMachine>, DbError> {
    Ok(sqlx::query_as("SELECT p.id AS provider_id,COALESCE(h.machine_id,p.machine_id) AS machine_id,
            CASE WHEN r.id IS NOT NULL THEN jsonb_build_object('lifecycle','unit') || COALESCE(h.drivers -> r.driver,'{}'::jsonb) ELSE COALESCE(m.facts -> 'provider_runtime_descriptor',m.facts -> 'environment_descriptor') END AS descriptor,
            m.facts -> 'policy' AS policy
        FROM providers p
        LEFT JOIN LATERAL (SELECT r.id,r.host_id,r.driver FROM runtimes r
            WHERE r.provider_id=p.id AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL
            ORDER BY r.created_at DESC LIMIT 1) r ON true
        LEFT JOIN hosts h ON h.id=r.host_id
        LEFT JOIN machines m ON m.id=COALESCE(h.machine_id,p.machine_id)
        WHERE p.id=ANY($1) AND p.deleted_at IS NULL")
        .bind(ids).fetch_all(conn).await?)
}

/// A prospective machine's directory services or a named driver, from its own declarations.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HostPortPolicy {
    /// The host facade.
    pub host_id: Id,
    /// Its physical machine.
    pub machine_id: Id,
    /// The host's familiar name.
    pub name: String,
    /// The canonical declaration reported by that machine, never the host page's cache.
    pub publish: Option<Json>,
    /// The acknowledgement revision used by current allocations.
    pub revision: i64,
}

/// Public declarations of these host facades from the same physical policy resolver.
pub async fn policies(conn: &mut Conn, machines: &[Id]) -> Result<Vec<(Id, Json)>, DbError> {
    Ok(sqlx::query_as("SELECT id,facts->'policy' FROM machines WHERE id=ANY($1) AND jsonb_typeof(facts->'policy')='object'")
        .bind(machines).fetch_all(conn).await?)
}

/// The physical publisher's one ports policy; a Host page is optional display metadata.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Publisher {
    /// The machine which owns the public listeners.
    pub machine_id: Id,
    /// Its Host page, only when it offers a node role.
    pub host_id: Option<Id>,
    /// Its familiar name.
    pub name: String,
    /// The reported canonical ports member, absent until reported.
    pub ports: Option<Json>,
}

/// Public listeners are machine services, equally available to directories and node units.
pub async fn publisher(conn: &mut Conn, machine: Id) -> Result<Option<Publisher>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.id AS machine_id,h.id AS host_id,COALESCE(h.name,m.name) AS name,
        NULLIF(m.facts#>'{policy,ports}','null'::jsonb) AS ports FROM machines m
        LEFT JOIN hosts h ON h.machine_id=m.id WHERE m.id=$1 AND m.status='active'",
    )
    .bind(machine)
    .fetch_optional(conn)
    .await?)
}

/// The current environment's public publisher follows the same physical-machine resolver.
pub async fn publisher_of(conn: &mut Conn, environment: Id) -> Result<Option<Publisher>, DbError> {
    let machine = of(conn, environment)
        .await?
        .and_then(|facts| facts.machine_id);
    match machine {
        Some(machine) => publisher(conn, machine).await,
        None => Ok(None),
    }
}

/// Public declarations of these host facades from the same physical policy resolver.
pub async fn host_ports(conn: &mut Conn, hosts: &[Id]) -> Result<Vec<HostPortPolicy>, DbError> {
    Ok(sqlx::query_as(
        "SELECT h.id AS host_id,h.machine_id,h.name,h.ports_revision AS revision,
        NULLIF(m.facts#>'{policy,ports,publish}','null'::jsonb) AS publish
        FROM hosts h JOIN machines m ON m.id=h.machine_id
        WHERE cardinality($1::uuid[])=0 OR h.id=ANY($1)",
    )
    .bind(hosts)
    .fetch_all(conn)
    .await?)
}

/// A prospective machine's directory services or a named driver, from its own declarations.
pub async fn on_machine(
    conn: &mut Conn,
    machine: Id,
    driver: Option<&str>,
) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar::<_, Option<Json>>(
        "SELECT CASE WHEN $2::text IS NULL THEN COALESCE(m.facts -> 'environment_descriptor',m.facts->'provider_runtime_descriptor')
                     ELSE (SELECT jsonb_build_object('lifecycle','unit') || (h.drivers -> $2) FROM hosts h WHERE h.machine_id=m.id LIMIT 1) END
           FROM machines m WHERE m.id=$1",
    ).bind(machine).bind(driver).fetch_optional(conn).await?.flatten())
}

/// Services and policy of a specific physical machine for a candidate attachment or unit.
pub async fn machine(
    conn: &mut Conn,
    id: Id,
    driver: Option<&str>,
) -> Result<Option<EnvironmentMachine>, DbError> {
    let selected: Option<(Id, Option<Json>, Option<String>)> = sqlx::query_as(
        "SELECT physical.id,physical.facts->'policy',r.driver
        FROM machines m LEFT JOIN runtimes r ON r.id=m.runtime_id
        LEFT JOIN hosts h ON h.id=COALESCE(r.host_id,m.host_id)
        JOIN machines physical ON physical.id=COALESCE(h.machine_id,m.id) WHERE m.id=$1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((physical, policy, unit_driver)) = selected else {
        return Ok(None);
    };
    let descriptor = on_machine(conn, physical, driver.or(unit_driver.as_deref())).await?;
    Ok(Some(EnvironmentMachine {
        machine_id: Some(physical),
        descriptor,
        policy,
    }))
}

/// The physical services and policy of a provider runtime, independently from its credential keeper.
pub async fn of_provider(conn: &mut Conn, id: Id) -> Result<Option<EnvironmentMachine>, DbError> {
    let selected:Option<(Id,Option<String>)>=sqlx::query_as("SELECT COALESCE(h.machine_id,p.machine_id),r.driver FROM providers p
        LEFT JOIN LATERAL (SELECT r.host_id,r.driver FROM runtimes r WHERE r.provider_id=p.id AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL ORDER BY r.created_at DESC LIMIT 1) r ON true
        LEFT JOIN hosts h ON h.id=r.host_id WHERE p.id=$1 AND COALESCE(h.machine_id,p.machine_id) IS NOT NULL")
        .bind(id).fetch_optional(&mut *conn).await?;
    match selected {
        Some((id, Some(driver))) => machine(conn, id, Some(&driver)).await,
        Some((id, None)) => {
            let row:Option<(Option<Json>,Option<Json>)>=sqlx::query_as("SELECT COALESCE(facts->'provider_runtime_descriptor',facts->'environment_descriptor'),facts->'policy' FROM machines WHERE id=$1")
                .bind(id).fetch_optional(conn).await?;
            Ok(row.map(|(descriptor, policy)| EnvironmentMachine {
                machine_id: Some(id),
                descriptor,
                policy,
            }))
        }
        None => Ok(None),
    }
}

/// The policy of the physical machine keeping the provider's credential, including a unit's parent.
pub async fn credential_policy(conn: &mut Conn, provider: Id) -> Result<Option<Json>, DbError> {
    Ok(credential_policies(conn, &[provider])
        .await?
        .into_iter()
        .next()
        .map(|(_, policy)| policy))
}

/// The same credential-keeper projection for a provider list, with one database read.
pub async fn credential_policies(
    conn: &mut Conn,
    providers: &[Id],
) -> Result<Vec<(Id, Json)>, DbError> {
    let rows:Vec<(Id,Option<Json>)>=sqlx::query_as("SELECT p.id,physical.facts->'policy' FROM providers p
        LEFT JOIN machines keeper ON keeper.id=p.key_machine_id
        LEFT JOIN LATERAL (SELECT r.id,r.host_id FROM runtimes r WHERE r.provider_id=p.id
            AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL ORDER BY r.created_at DESC LIMIT 1) r
            ON keeper.id IS NULL OR keeper.runtime_id=r.id
        LEFT JOIN hosts h ON h.id=COALESCE(keeper.host_id,r.host_id)
        JOIN machines physical ON physical.id=COALESCE(h.machine_id,keeper.id)
        WHERE p.id=ANY($1) AND p.deleted_at IS NULL")
        .bind(providers).fetch_all(conn).await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, policy)| policy.map(|policy| (id, policy)))
        .collect())
}

/// Whether an environment's physical machine explicitly permits a named policy switch.
pub async fn permits(conn: &mut Conn, environment: Id, key: &str) -> Result<bool, DbError> {
    Ok(of(conn, environment)
        .await?
        .and_then(|m| m.policy)
        .and_then(|p| p.get(key).and_then(Json::as_bool))
        == Some(true))
}

/// Whether the physical machine that holds an environment is known and does not permit a named
/// policy switch: its last report says no, or says nothing of it. While no machine is known to hold
/// it there is no local policy to refuse by; such an environment cannot be reached either.
pub async fn refuses(conn: &mut Conn, environment: Id, key: &str) -> Result<bool, DbError> {
    Ok(of(conn, environment).await?.is_some_and(|machine| {
        machine.machine_id.is_some()
            && machine
                .policy
                .as_ref()
                .and_then(|policy| policy.get(key))
                .and_then(Json::as_bool)
                != Some(true)
    }))
}

/// Whether the resource has a unit recipe and therefore starts and stops a unit.
pub async fn has_unit(conn: &mut Conn, environment: Id) -> Result<bool, DbError> {
    Ok(of(conn, environment)
        .await?
        .and_then(|m| m.descriptor)
        .and_then(|d| d.get("lifecycle").and_then(Json::as_str).map(str::to_owned))
        .as_deref()
        == Some("unit"))
}

/// Physical ownership sources of connected directory environments. Managed units keep
/// their own ownership semantics; a node host uses its existing canonical host facade.
pub async fn ownership_sources(
    conn: &mut Conn,
    environments: &[Id],
) -> Result<Vec<(Id, String, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT e.id,CASE WHEN h.id IS NULL THEN 'machine' ELSE 'host' END,COALESCE(h.id,m.id)
         FROM environments e JOIN machines m ON m.id=e.machine_id
         LEFT JOIN hosts h ON h.machine_id=m.id
         WHERE e.id=ANY($1) AND e.placement='connected' AND e.template_id IS NULL
           AND e.deleted_at IS NULL AND m.placement='connected' AND m.runtime_id IS NULL
           AND m.deleted_at IS NULL AND m.status='active'",
    )
    .bind(environments)
    .fetch_all(conn)
    .await?)
}
