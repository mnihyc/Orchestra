//! Native pool metadata, paged written members and serialized graph edits.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
/// Exact offering identity used by membership fixtures and native model fact suppliers.
pub use orch_provider_contract::capabilities::{BackendRef, OfferingRef};
/// Canonical identities consumed by the native membership query and its authority checker.
pub use orch_provider_contract::pools::{Leaf, MemberRule};
use orch_types::{Id, Json};

/// A pool's own stored identity and settings; no expanded member names are attached.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Row {
    pub id: Id,
    pub org_id: Id,
    pub home_id: Id,
    pub owner_id: Id,
    pub name: String,
    pub description: String,
    pub kind: String,
    pub definition: Json,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub deleted_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "id,org_id,home_id,owner_id,name,description,kind,definition,revision,created_at,updated_at,deleted_at";

/// One pool, without turning its stored identifiers into outward references.
pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<Row>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM pools WHERE id=$1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// An already authorized set, read together.
pub async fn among(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Row>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM pools WHERE id=ANY($1) ORDER BY id"
    )))
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// One candidate window; ordinary native authority filters the returned identities before disclosure.
pub async fn candidates(
    conn: &mut Conn,
    root: Id,
    kind: &str,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<Row>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM pools
        WHERE org_id=$1 AND kind=$2 AND deleted_at IS NULL AND ($3::uuid IS NULL OR id>$3)
        ORDER BY id LIMIT $4"
    )))
    .bind(root)
    .bind(kind)
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// A written rule with its stable ordering cursor; numeric absence follows registry defaults.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Member {
    pub id: Id,
    pub position: i64,
    pub rule: Json,
    pub enabled: bool,
    pub priority: Option<i64>,
    pub weight: Option<String>,
}

/// Read only the next page, rather than building the expanded graph in memory.
pub async fn members(
    conn: &mut Conn,
    pool: Id,
    after: Option<(i64, Id)>,
    limit: i64,
) -> Result<Vec<Member>, DbError> {
    let (position, id) = after.map_or((None, None), |(position, id)| (Some(position), Some(id)));
    Ok(sqlx::query_as("SELECT id,position,rule,enabled,priority,weight FROM pool_members
        WHERE pool_id=$1 AND ($2::bigint IS NULL OR (position,id)>($2,$3)) ORDER BY position,id LIMIT $4")
        .bind(pool).bind(position).bind(id).bind(limit).fetch_all(conn).await?)
}

/// Serialize graph edits before individual locks, including edges between shared roots.
/// The native guarded writer owns the surrounding transaction.
pub async fn lock_graph(conn: &mut Conn) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('resource-pool-graph',0))")
        .execute(conn)
        .await?;
    Ok(())
}

/// Pin the actual stored definition and revision after its graph namespace has been locked.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<Row>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM pools WHERE id=$1 FOR UPDATE"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Check an inner reference under the same graph lock; no cycle or cross-family reference is usable.
pub async fn check_child(conn: &mut Conn, parent: &Row, child: Id) -> Result<(), DbError> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pools WHERE id=$1 AND kind=$2 AND deleted_at IS NULL)",
    )
    .bind(child)
    .bind(&parent.kind)
    .fetch_one(&mut *conn)
    .await?;
    if !valid {
        return Err(DbError::Rule {
            constraint: "pool_member_kind".into(),
        });
    }
    let cycle: bool = sqlx::query_scalar("WITH RECURSIVE descendants(id,seen) AS (
        SELECT $1::uuid,ARRAY[$1::uuid]
        UNION ALL SELECT e.child_id,d.seen || e.child_id FROM descendants d JOIN pool_edges e ON e.pool_id=d.id
        WHERE NOT e.child_id=ANY(d.seen)
        ) SELECT EXISTS(SELECT 1 FROM descendants WHERE id=$2)")
        .bind(child).bind(parent.id).fetch_one(conn).await?;
    if cycle {
        return Err(DbError::Rule {
            constraint: "pool_members_no_cycle".into(),
        });
    }
    Ok(())
}

/// Insert the native pool identity after its original creation authority's first-effect fence.
pub async fn insert(conn: &mut Conn, row: &Row) -> Result<(), DbError> {
    sqlx::query("INSERT INTO pools(id,org_id,home_id,owner_id,name,description,kind,definition,created_at,updated_at)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(row.id).bind(row.org_id).bind(row.home_id).bind(row.owner_id).bind(&row.name)
        .bind(&row.description).bind(&row.kind).bind(&row.definition).bind(row.created_at).bind(row.updated_at)
        .execute(conn).await?;
    Ok(())
}

/// Copies written rules under the caller's held graph/source/destination footprint.
/// New member identities retain their authored positions and raw rules without a hydrated target list.
pub async fn copy_members(conn: &mut Conn, source: Id, destination: Id) -> Result<(), DbError> {
    sqlx::query("INSERT INTO pool_members(pool_id,id,position,rule,enabled,priority,weight) SELECT $2,gen_random_uuid(),position,rule,enabled,priority,weight FROM pool_members WHERE pool_id=$1")
        .bind(source).bind(destination).execute(&mut *conn).await?;
    sqlx::query("INSERT INTO pool_edges(pool_id,member_id,child_id) SELECT pool_id,id,(rule->>'id')::uuid FROM pool_members WHERE pool_id=$1 AND rule->>'type'='pool'")
        .bind(destination).execute(conn).await?;
    Ok(())
}

/// Replace metadata under the caller's revision and immutable member family.
pub async fn update(conn: &mut Conn, row: &Row, expected: i64) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE pools SET name=$3,description=$4,definition=$5,revision=revision+1,updated_at=$6
        WHERE id=$1 AND revision=$2 AND kind=$7 AND deleted_at IS NULL",
    )
    .bind(row.id)
    .bind(expected)
    .bind(&row.name)
    .bind(&row.description)
    .bind(&row.definition)
    .bind(row.updated_at)
    .bind(&row.kind)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Save one written rule and its real graph edge in the guarded writer's transaction.
pub async fn put_member(
    conn: &mut Conn,
    parent: &Row,
    member: &Member,
    child: Option<Id>,
) -> Result<(), DbError> {
    if let Some(child) = child {
        check_child(conn, parent, child).await?;
    }
    sqlx::query("INSERT INTO pool_members(pool_id,id,position,rule,enabled,priority,weight) VALUES($1,$2,$3,$4,$5,$6,$7)
        ON CONFLICT(pool_id,id) DO UPDATE SET position=EXCLUDED.position,rule=EXCLUDED.rule,
            enabled=EXCLUDED.enabled,priority=EXCLUDED.priority,weight=EXCLUDED.weight")
        .bind(parent.id).bind(member.id).bind(member.position).bind(&member.rule).bind(member.enabled)
        .bind(member.priority).bind(&member.weight).execute(&mut *conn).await?;
    sqlx::query("DELETE FROM pool_edges WHERE pool_id=$1 AND member_id=$2")
        .bind(parent.id)
        .bind(member.id)
        .execute(&mut *conn)
        .await?;
    if let Some(child) = child {
        sqlx::query("INSERT INTO pool_edges(pool_id,member_id,child_id) VALUES($1,$2,$3)")
            .bind(parent.id)
            .bind(member.id)
            .bind(child)
            .execute(conn)
            .await?;
    }
    Ok(())
}

/// Remove exactly one reviewed member; no expanded resources are deleted.
pub async fn remove_member(conn: &mut Conn, pool: Id, member: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM pool_edges WHERE pool_id=$1 AND member_id=$2")
        .bind(pool)
        .bind(member)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM pool_members WHERE pool_id=$1 AND id=$2")
        .bind(pool)
        .bind(member)
        .execute(conn)
        .await?;
    Ok(())
}

/// Explicit member-side consent, retaining its giver and typed options for every current check.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Lending {
    pub id: Id,
    pub resource_kind: String,
    pub resource_id: Id,
    pub giver_id: Id,
    pub matcher: Json,
    pub options: Json,
    pub show_names: bool,
    pub revision: i64,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub renewed_at: Option<DateTime<Utc>>,
}

/// Current consents only; their giver and complete requested options still need native revalidation.
pub async fn lending(conn: &mut Conn, kind: &str, id: Id) -> Result<Vec<Lending>, DbError> {
    Ok(sqlx::query_as("SELECT id,resource_kind,resource_id,giver_id,matcher,options,show_names,revision,created_at,expires_at,renewed_at
        FROM pool_lending WHERE resource_kind=$1 AND resource_id=$2 AND removed_at IS NULL ORDER BY id")
        .bind(kind).bind(id).fetch_all(conn).await?)
}

/// One current written member, re-read at native admission rather than trusted from a choice receipt.
pub async fn member(conn: &mut Conn, pool: Id, id: Id) -> Result<Option<Member>, DbError> {
    Ok(sqlx::query_as("SELECT id,position,rule,enabled,priority,weight FROM pool_members WHERE pool_id=$1 AND id=$2")
        .bind(pool).bind(id).fetch_optional(conn).await?)
}

/// Membership facts shared by expansion, duplicate detection and the native lending fence.
/// This asks no workspace filesystem and grants no authority.
pub async fn rule_contains(
    conn: &mut Conn,
    root: Id,
    rule: &orch_provider_contract::pools::MemberRule,
    leaf: &orch_provider_contract::pools::Leaf,
) -> Result<bool, DbError> {
    use orch_provider_contract::pools::{Kind, Leaf, MemberRule, Target};
    match rule {
        MemberRule::Single { resource } => {
            let identity = |leaf: &Leaf| {
                Target::Single {
                    resource: leaf.clone(),
                }
                .identity()
                .map_err(|error| DbError::Other(sqlx::Error::Decode(error.into())))
            };
            Ok(identity(resource)? == identity(leaf)?)
        }
        MemberRule::Pool { .. } => Ok(false),
        MemberRule::TemplateOnHost {
            template_id,
            host_id,
        } => Ok(
            matches!(leaf,Leaf::EnvironmentTemplate{template_id:t,host_id:h}if t==template_id&&h==host_id),
        ),
        MemberRule::ProviderModels { provider_id, group } => {
            let Leaf::Model { backend } = leaf else {
                return Ok(false);
            };
            let Some(offering) = backend
                .offering
                .as_ref()
                .filter(|offering| offering.provider == *provider_id)
            else {
                return Ok(false);
            };
            Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_models WHERE provider_id=$1 AND model_name=$2 AND enabled AND ($3::text IS NULL OR $3=ANY(groups)))")
                .bind(provider_id).bind(&offering.offering).bind(group).fetch_one(conn).await?)
        }
        MemberRule::HostEnvironments { host_id } => {
            let Leaf::Environment { id } = leaf else {
                return Ok(false);
            };
            Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM environments e WHERE e.id=$1 AND e.deleted_at IS NULL AND (
                EXISTS(SELECT 1 FROM runtimes r WHERE r.environment_id=e.id AND r.host_id=$2 AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed'))
                OR EXISTS(SELECT 1 FROM hosts h WHERE h.id=$2 AND h.machine_id=e.machine_id)))")
                .bind(id).bind(host_id).fetch_one(conn).await?)
        }
        MemberRule::Grouping { kind, group } => {
            if *kind != leaf.kind() {
                return Ok(false);
            }
            let (family, id) = match leaf {
                Leaf::Model { backend } => {
                    let Some(offering) = backend.offering.as_ref() else {
                        return Ok(false);
                    };
                    let enabled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_models WHERE provider_id=$1 AND model_name=$2 AND enabled)").bind(offering.provider).bind(&offering.offering).fetch_one(&mut *conn).await?;
                    if !enabled {
                        return Ok(false);
                    }
                    ("provider", offering.provider)
                }
                Leaf::Provider { id } => ("provider", *id),
                Leaf::Environment { id } => ("environment", *id),
                Leaf::Host { id } => ("host", *id),
                Leaf::EnvironmentTemplate { .. } => return Ok(false),
            };
            let rows = super::resources::grouping::among(conn, &[(family.into(), id)]).await?;
            let Some(row) = rows.first() else {
                return Ok(false);
            };
            Ok(match group {
                orch_provider_contract::pools::DisplayGroup::Home{id}=>row.home_id==*id,
                orch_provider_contract::pools::DisplayGroup::Owner{owner_id,home_id}=>row.owner_id==*owner_id && (owner_id.is_some()||row.home_id==*home_id),
                orch_provider_contract::pools::DisplayGroup::Machine{id}=>if let Some(id)=id{
                    sqlx::query_scalar("SELECT COALESCE($1::uuid=$2::uuid,false) OR EXISTS(SELECT 1 FROM machines WHERE id=$1 AND host_id=$2)").bind(row.origin_machine_id).bind(id).fetch_one(conn).await?
                }else{row.origin_machine_id.is_none()},
                orch_provider_contract::pools::DisplayGroup::Tag{id:group_id}=>sqlx::query_scalar("SELECT CASE WHEN $3::uuid IS NULL THEN NOT EXISTS(SELECT 1 FROM resource_tags WHERE resource_kind=$1 AND resource_id=$2) ELSE EXISTS(SELECT 1 FROM resource_tags WHERE resource_kind=$1 AND resource_id=$2 AND tag_id=$3) END")
                    .bind(family).bind(id).bind(group_id).fetch_one(conn).await?,
            })
        }
        MemberRule::Matching {
            kind,
            home_id,
            tag_id,
            owner_id,
            machine_id,
        } => {
            if *kind != leaf.kind() {
                return Ok(false);
            }
            let (family, id) = match leaf {
                Leaf::Model { backend } => {
                    let Some(offering) = backend.offering.as_ref() else {
                        return Ok(false);
                    };
                    let enabled:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM provider_models WHERE provider_id=$1 AND model_name=$2 AND enabled)")
                        .bind(offering.provider).bind(&offering.offering).fetch_one(&mut *conn).await?;
                    if !enabled {
                        return Ok(false);
                    }
                    ("provider", offering.provider)
                }
                Leaf::Provider { id } => ("provider", *id),
                Leaf::Host { id } => ("host", *id),
                Leaf::Environment { id } => ("environment", *id),
                Leaf::EnvironmentTemplate { .. } => return Ok(false),
            };
            let found = super::resources::resource_facts(conn, &[id]).await?;
            let Some(resource) = found
                .iter()
                .find(|row| row.id == id && row.kind == family && !row.deleted)
            else {
                return Ok(false);
            };
            if owner_id.is_some_and(|owner| resource.owner_id != Some(owner)) {
                return Ok(false);
            }
            if let Some(home) = home_id {
                let matches:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM nodes WHERE id=$1 AND $2=ANY(path) AND deleted_at IS NULL)")
                    .bind(resource.home_id).bind(home).fetch_one(&mut *conn).await?;
                if !matches {
                    return Ok(false);
                }
            } else if resource.org_id != Some(root) {
                return Ok(false);
            }
            if let Some(tag) = tag_id {
                let matches:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM resource_tags WHERE resource_kind=$1 AND resource_id=$2 AND tag_id=$3)")
                    .bind(family).bind(id).bind(tag).fetch_one(&mut *conn).await?;
                if !matches {
                    return Ok(false);
                }
            }
            if let Some(machine) = machine_id {
                let grouping =
                    super::resources::grouping::among(conn, &[(family.into(), id)]).await?;
                let origin = grouping.first().and_then(|row| row.origin_machine_id);
                let matches:bool=sqlx::query_scalar("SELECT COALESCE($1::uuid=$2::uuid,false) OR EXISTS(SELECT 1 FROM machines WHERE id=$1 AND host_id=$2)")
                    .bind(origin).bind(machine).fetch_one(conn).await?;
                if !matches {
                    return Ok(false);
                }
            }
            Ok(matches!(
                kind,
                Kind::Model | Kind::Provider | Kind::Environment | Kind::Host
            ))
        }
    }
}

/// Consents on exactly the native inherited/locked policy origins of the consuming Use row.
pub async fn lending_for(
    conn: &mut Conn,
    kind: &str,
    scopes: &[(bool, Id)],
    now: DateTime<Utc>,
) -> Result<Vec<Lending>, DbError> {
    let (defaults, ids): (Vec<_>, Vec<_>) = scopes.iter().copied().unzip();
    Ok(sqlx::query_as("SELECT l.id,l.resource_kind,l.resource_id,l.giver_id,l.matcher,l.options,l.show_names,l.revision,l.created_at,l.expires_at,l.renewed_at
        FROM pool_lending l JOIN UNNEST($2::boolean[],$3::uuid[]) wanted(is_defaults,id)
        ON l.is_defaults=wanted.is_defaults AND l.resource_id=wanted.id
        WHERE l.resource_kind=$1 AND l.removed_at IS NULL AND (l.expires_at IS NULL OR l.expires_at>$4) ORDER BY l.id")
        .bind(kind).bind(defaults).bind(ids).bind(now).fetch_all(conn).await?)
}

/// One provider-local model identity; its consuming native factory supplies the exact backend.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ModelIdentity {
    pub provider_id: Id,
    pub model_name: String,
}

/// Model rule expansion stays paged and uses only indexed resource/model metadata.
pub async fn model_candidates(
    conn: &mut Conn,
    root: Id,
    rule: &MemberRule,
    after: Option<(Id, String)>,
    limit: i64,
) -> Result<Vec<ModelIdentity>, DbError> {
    if let MemberRule::Grouping {
        kind: orch_provider_contract::pools::Kind::Model,
        group,
    } = rule
    {
        return grouping_models(conn, group, after, limit).await;
    }
    let (provider, group, home, tag, owner, machine) = match rule {
        MemberRule::ProviderModels { provider_id, group } => {
            (Some(*provider_id), group.clone(), None, None, None, None)
        }
        MemberRule::Matching {
            kind: orch_provider_contract::pools::Kind::Model,
            home_id,
            tag_id,
            owner_id,
            machine_id,
        } => (None, None, *home_id, *tag_id, *owner_id, *machine_id),
        _ => return Ok(Vec::new()),
    };
    let (after_provider, after_model) = after.map_or((None, None), |(provider, model)| {
        (Some(provider), Some(model))
    });
    Ok(sqlx::query_as("SELECT pm.provider_id,pm.model_name FROM provider_models pm JOIN providers p ON p.id=pm.provider_id
        LEFT JOIN nodes n ON n.id=p.home_id WHERE pm.enabled AND p.deleted_at IS NULL
        AND ($1::uuid IS NULL OR p.id=$1) AND ($2::text IS NULL OR $2=ANY(pm.groups))
        AND ($1::uuid IS NOT NULL OR CASE WHEN $3::uuid IS NULL THEN p.org_id=$4 ELSE $3=ANY(n.path) END)
        AND ($5::uuid IS NULL OR EXISTS(SELECT 1 FROM resource_tags t WHERE t.resource_kind='provider' AND t.resource_id=p.id AND t.tag_id=$5))
        AND ($6::uuid IS NULL OR p.owner_id=$6)
        AND ($7::uuid IS NULL OR COALESCE(p.key_machine_id,p.machine_id)=$7 OR EXISTS(SELECT 1 FROM machines m WHERE m.id=COALESCE(p.key_machine_id,p.machine_id) AND m.host_id=$7))
        AND ($8::uuid IS NULL OR (pm.provider_id,pm.model_name)>($8,$9)) ORDER BY pm.provider_id,pm.model_name LIMIT $10")
        .bind(provider).bind(group).bind(home).bind(root).bind(tag).bind(owner).bind(machine).bind(after_provider).bind(after_model).bind(limit)
        .fetch_all(conn).await?)
}

/// Homogeneous live metadata rules return resource identities, without loading source directories.
pub async fn resource_candidates(
    conn: &mut Conn,
    root: Id,
    rule: &MemberRule,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    use orch_provider_contract::pools::Kind;
    if let MemberRule::Grouping { kind, group } = rule {
        return grouping_resources(conn, *kind, group, after, limit).await;
    }
    if let MemberRule::HostEnvironments { host_id } = rule {
        return Ok(sqlx::query_scalar("SELECT e.id FROM environments e WHERE e.deleted_at IS NULL AND ($2::uuid IS NULL OR e.id>$2)
            AND (EXISTS(SELECT 1 FROM runtimes r WHERE r.environment_id=e.id AND r.host_id=$1 AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed'))
            OR EXISTS(SELECT 1 FROM hosts h WHERE h.id=$1 AND h.machine_id=e.machine_id)) ORDER BY e.id LIMIT $3")
            .bind(host_id).bind(after).bind(limit).fetch_all(conn).await?);
    }
    let MemberRule::Matching {
        kind,
        home_id,
        tag_id,
        owner_id,
        machine_id,
    } = rule
    else {
        return Ok(Vec::new());
    };
    let family = match kind {
        Kind::Provider => "provider",
        Kind::Environment => "environment",
        Kind::Host => "host",
        Kind::Model => return Ok(Vec::new()),
    };
    let facts = super::resources::facts_select(family).ok_or_else(|| DbError::Rule {
        constraint: "pool_member_kind".into(),
    })?;
    let origin = if family == "provider" {
        "(SELECT COALESCE(p.key_machine_id,p.machine_id) FROM providers p WHERE p.id=f.id)"
    } else {
        "f.machine_id"
    };
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT f.id FROM ({facts}) f LEFT JOIN nodes n ON n.id=f.home_id
        WHERE NOT f.deleted AND CASE WHEN $1::uuid IS NULL THEN f.org_id=$2 ELSE $1=ANY(n.path) END
        AND ($3::uuid IS NULL OR EXISTS(SELECT 1 FROM resource_tags t WHERE t.resource_kind=$4 AND t.resource_id=f.id AND t.tag_id=$3))
        AND ($5::uuid IS NULL OR f.owner_id=$5)
        AND ($6::uuid IS NULL OR {origin}=$6 OR EXISTS(SELECT 1 FROM machines m WHERE m.id={origin} AND m.host_id=$6))
        AND ($7::uuid IS NULL OR f.id>$7) ORDER BY f.id LIMIT $8")))
        .bind(home_id).bind(root).bind(tag_id).bind(family).bind(owner_id).bind(machine_id).bind(after).bind(limit).fetch_all(conn).await?)
}

/// Indexed exact occurrences keep large explicit member lists out of a quadratic duplicate walk.
pub async fn first_exact(
    conn: &mut Conn,
    pool: Id,
    leaf: &Leaf,
) -> Result<Option<(i64, Id)>, DbError> {
    let value = serde_json::to_value(leaf)
        .map_err(|error| DbError::Other(sqlx::Error::Decode(error.into())))?;
    Ok(sqlx::query_as("SELECT position,id FROM pool_members WHERE pool_id=$1 AND enabled AND rule->>'type'='single'
        AND md5((rule->'resource')::text)=md5($2::jsonb::text) AND rule->'resource'=$2 ORDER BY position,id LIMIT 1")
        .bind(pool).bind(value).fetch_optional(conn).await?)
}

/// Dynamic rules can overlap; exact rules are checked separately through their identity index.
pub async fn dynamic_members(
    conn: &mut Conn,
    pool: Id,
    before: (i64, Id),
    after: Option<(i64, Id)>,
    limit: i64,
) -> Result<Vec<Member>, DbError> {
    let (position, id) = after.map_or((None, None), |(position, id)| (Some(position), Some(id)));
    Ok(sqlx::query_as("SELECT id,position,rule,enabled,priority,weight FROM pool_members WHERE pool_id=$1 AND enabled
        AND rule->>'type' NOT IN ('single','pool') AND (position,id)<($2,$3)
        AND ($4::bigint IS NULL OR (position,id)>($4,$5)) ORDER BY position,id LIMIT $6")
        .bind(pool).bind(before.0).bind(before.1).bind(position).bind(id).bind(limit).fetch_all(conn).await?)
}

fn grouping_parameters(
    group: &orch_provider_contract::pools::DisplayGroup,
) -> (&'static str, Option<Id>, Option<Id>) {
    use orch_provider_contract::pools::DisplayGroup;
    match group {
        DisplayGroup::Home { id } => ("home", *id, None),
        DisplayGroup::Owner { owner_id, home_id } => ("owner", *owner_id, *home_id),
        DisplayGroup::Machine { id } => ("machine", *id, None),
        DisplayGroup::Tag { id } => ("tag", *id, None),
    }
}
fn grouping_predicate(origin: &str) -> String {
    format!("CASE $1::text
    WHEN 'home' THEN f.home_id IS NOT DISTINCT FROM $2::uuid
    WHEN 'owner' THEN f.owner_id IS NOT DISTINCT FROM $2::uuid AND ($2::uuid IS NOT NULL OR f.home_id IS NOT DISTINCT FROM $3::uuid)
    WHEN 'machine' THEN {origin} IS NOT DISTINCT FROM $2::uuid OR EXISTS(SELECT 1 FROM machines m WHERE m.id={origin} AND m.host_id=$2)
    WHEN 'tag' THEN CASE WHEN $2::uuid IS NULL THEN NOT EXISTS(SELECT 1 FROM resource_tags t WHERE t.resource_kind=$4 AND t.resource_id=f.id)
        ELSE EXISTS(SELECT 1 FROM resource_tags t WHERE t.resource_kind=$4 AND t.resource_id=f.id AND t.tag_id=$2) END ELSE false END")
}
async fn grouping_resources(
    conn: &mut Conn,
    kind: orch_provider_contract::pools::Kind,
    group: &orch_provider_contract::pools::DisplayGroup,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    use orch_provider_contract::pools::Kind;
    let family = match kind {
        Kind::Provider => "provider",
        Kind::Environment => "environment",
        Kind::Host => "host",
        Kind::Model => return Ok(Vec::new()),
    };
    let facts = super::resources::facts_select(family).ok_or_else(|| DbError::Rule {
        constraint: "pool_member_kind".into(),
    })?;
    let origin = if family == "provider" {
        "(SELECT COALESCE(p.key_machine_id,p.machine_id) FROM providers p WHERE p.id=f.id)"
    } else {
        "f.machine_id"
    };
    let predicate = grouping_predicate(origin);
    let (mode, id, home) = grouping_parameters(group);
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT f.id FROM ({facts}) f WHERE NOT f.deleted AND ({predicate}) AND ($5::uuid IS NULL OR f.id>$5) ORDER BY f.id LIMIT $6")))
        .bind(mode).bind(id).bind(home).bind(family).bind(after).bind(limit).fetch_all(conn).await?)
}
async fn grouping_models(
    conn: &mut Conn,
    group: &orch_provider_contract::pools::DisplayGroup,
    after: Option<(Id, String)>,
    limit: i64,
) -> Result<Vec<ModelIdentity>, DbError> {
    let facts = super::resources::facts_select("provider").ok_or_else(|| DbError::Rule {
        constraint: "pool_member_kind".into(),
    })?;
    let predicate = grouping_predicate(
        "(SELECT COALESCE(p.key_machine_id,p.machine_id) FROM providers p WHERE p.id=f.id)",
    );
    let (mode, id, home) = grouping_parameters(group);
    let (after_provider, after_model) = after.map_or((None, None), |(provider, model)| {
        (Some(provider), Some(model))
    });
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT pm.provider_id,pm.model_name FROM ({facts}) f JOIN provider_models pm ON pm.provider_id=f.id
        WHERE NOT f.deleted AND pm.enabled AND ({predicate}) AND ($5::uuid IS NULL OR (pm.provider_id,pm.model_name)>($5,$6)) ORDER BY pm.provider_id,pm.model_name LIMIT $7")))
        .bind(mode).bind(id).bind(home).bind("provider").bind(after_provider).bind(after_model).bind(limit).fetch_all(conn).await?)
}

/// Live location sets use these same saved membership predicates, without selection or health.
/// Only rule metadata is paged; neither directory contents nor expanded candidate lists are read.
pub async fn structural_pools(
    conn: &mut Conn,
    leaf: &Leaf,
    batch: i64,
) -> Result<Vec<Id>, DbError> {
    let mut direct = std::collections::BTreeSet::new();
    let mut after: Option<(Id, Id)> = None;
    loop {
        let (pool, member) =
            after.map_or((None, None), |(pool, member)| (Some(pool), Some(member)));
        let page: Vec<(Id,Id,Id,Json)> = sqlx::query_as("SELECT p.id,m.id,p.org_id,m.rule FROM pool_members m JOIN pools p ON p.id=m.pool_id WHERE p.deleted_at IS NULL AND p.kind=$1 AND m.enabled AND m.rule->>'type'<>'pool' AND ($2::uuid IS NULL OR (p.id,m.id)>($2,$3)) ORDER BY p.id,m.id LIMIT $4")
            .bind(leaf.kind().as_str()).bind(pool).bind(member).bind(batch).fetch_all(&mut *conn).await?;
        if page.is_empty() {
            break;
        }
        after = page.last().map(|row| (row.0, row.1));
        for (pool, _, root, rule) in page {
            let rule: MemberRule = serde_json::from_value(rule).map_err(|_| DbError::Rule {
                constraint: "pool_member_rule".into(),
            })?;
            if rule_contains(conn, root, &rule, leaf).await? {
                direct.insert(pool);
            }
        }
    }
    let direct: Vec<_> = direct.into_iter().collect();
    Ok(sqlx::query_scalar("WITH RECURSIVE members(id) AS (SELECT unnest($1::uuid[]) UNION SELECT e.pool_id FROM members found JOIN pool_edges e ON e.child_id=found.id JOIN pool_members m ON m.pool_id=e.pool_id AND m.id=e.member_id JOIN pools p ON p.id=e.pool_id WHERE p.deleted_at IS NULL AND p.kind=$2 AND m.enabled) SELECT id FROM members ORDER BY id")
        .bind(direct).bind(leaf.kind().as_str()).fetch_all(conn).await?)
}
