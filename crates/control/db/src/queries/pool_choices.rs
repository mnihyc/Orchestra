//! Durable choice facts and rotation state, never permission or native execution receipts.
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Choice {
    pub actor_id: Id,
    pub source: Json,
    pub target: Json,
    pub state: String,
    pub leaf: Option<Json>,
    pub path: Option<Json>,
    pub reason: Option<String>,
}
/// Read a retained choice without advancing, rebuilding or reserving it.
pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<Choice>, DbError> {
    Ok(sqlx::query_as(
        "SELECT actor_id,source,target,state,leaf,path,reason FROM pool_choices WHERE id=$1",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}
pub async fn begin(
    conn: &mut Conn,
    id: Id,
    actor: Id,
    source: &Json,
    target: &Json,
    now: DateTime<Utc>,
) -> Result<Choice, DbError> {
    sqlx::query("INSERT INTO pool_choices(id,actor_id,source,target,state,created_at,updated_at) VALUES($1,$2,$3,$4,'building',$5,$5) ON CONFLICT(id) DO NOTHING").bind(id).bind(actor).bind(source).bind(target).bind(now).execute(&mut *conn).await?;
    Ok(sqlx::query_as("SELECT actor_id,source,target,state,leaf,path,reason FROM pool_choices WHERE id=$1 FOR UPDATE").bind(id).fetch_one(conn).await?)
}
pub struct Node<'a> {
    pub id: Id,
    pub pool: Id,
    pub parent: Option<Id>,
    pub member: Option<Id>,
    pub priority: u64,
    pub weight: &'a str,
    pub ordinal: i64,
    pub policy: &'a Json,
    pub revision: i64,
    pub definition: &'a Json,
    pub defaults: &'a Json,
}
pub async fn node(conn: &mut Conn, choice: Id, node: &Node<'_>) -> Result<(), DbError> {
    sqlx::query("INSERT INTO pool_choice_nodes(choice_id,id,pool_id,parent_id,member_id,priority,weight,ordinal,policy,revision,definition,defaults) VALUES($1,$2,$3,$4,$5,$6::text::numeric,$7,$8,$9,$10,$11,$12)")
        .bind(choice).bind(node.id).bind(node.pool).bind(node.parent).bind(node.member).bind(node.priority.to_string()).bind(node.weight).bind(node.ordinal).bind(node.policy).bind(node.revision).bind(node.definition).bind(node.defaults).execute(conn).await?;
    Ok(())
}
pub struct Member<'a> {
    pub ordinal: i64,
    pub parent: Id,
    pub member: Id,
    pub priority: u64,
    pub weight: &'a str,
    pub leaf: &'a Json,
    pub path: &'a Json,
    pub reason: Option<&'a str>,
    pub cost: Option<&'a str>,
    pub measurement: &'a Json,
    pub loads: &'a Json,
}
pub async fn member(
    conn: &mut Conn,
    choice: Id,
    member: &Member<'_>,
    ancestors: &[Id],
) -> Result<(), DbError> {
    let duplicate: bool = if member.reason.is_none() {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pool_choice_members WHERE choice_id=$1 AND leaf_hash=md5($2::jsonb::text) AND leaf=$2 AND reason IS NULL)").bind(choice).bind(member.leaf).fetch_one(&mut *conn).await?
    } else {
        false
    };
    let reason = if duplicate {
        Some("duplicate_member")
    } else {
        member.reason
    };
    sqlx::query("INSERT INTO pool_choice_members(choice_id,ordinal,parent_id,member_id,priority,weight,leaf,path,reason,cost,measurement,loads) VALUES($1,$2,$3,$4,$5::text::numeric,$6,$7,$8,$9,$10::text::numeric,$11,$12)")
        .bind(choice).bind(member.ordinal).bind(member.parent).bind(member.member).bind(member.priority.to_string()).bind(member.weight).bind(member.leaf).bind(member.path).bind(reason).bind(member.cost).bind(member.measurement).bind(member.loads).execute(&mut *conn).await?;
    if reason.is_none() {
        sqlx::query("UPDATE pool_choice_nodes SET feasible=true WHERE choice_id=$1 AND id=ANY($2) AND NOT feasible").bind(choice).bind(ancestors).execute(conn).await?;
    }
    Ok(())
}
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Edge {
    pub ordinal: i64,
    pub weight: String,
    pub child: Option<Id>,
    pub leaf: Option<Json>,
    pub path: Option<Json>,
    pub identity: Json,
    pub cost: Option<String>,
    pub loads: Json,
}
const EDGES: &str = "SELECT n.ordinal,n.weight,n.id child,NULL::jsonb leaf,NULL::jsonb path,jsonb_build_object('pool',n.pool_id) identity,n.priority,n.cost,n.loads FROM pool_choice_nodes n WHERE n.choice_id=$1 AND n.parent_id=$2 AND n.feasible UNION ALL SELECT m.ordinal,m.weight,NULL::uuid,m.leaf,m.path,jsonb_build_object('leaf',m.leaf),m.priority,m.cost,m.loads FROM pool_choice_members m WHERE m.choice_id=$1 AND m.parent_id=$2 AND m.reason IS NULL";
const RANKED: &str = "SELECT * FROM edges WHERE priority=(SELECT min(priority) FROM edges)";
pub struct Ranking<'a> {
    pub mode: &'a str,
    pub measure: Option<&'a str>,
    pub ceiling: Option<f64>,
}
pub async fn edges(
    conn: &mut Conn,
    choice: Id,
    parent: Id,
    after: Option<i64>,
    limit: i64,
    ranking: &Ranking<'_>,
) -> Result<Vec<Edge>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("WITH raw_edges AS ({EDGES}),edges AS(SELECT *,(loads->$6->>'score')::double precision measured FROM raw_edges WHERE $5<>'load' OR ((loads->$6->>'score')::double precision IS NOT NULL AND ($7::double precision IS NULL OR (loads->$6->>'score')::double precision<=$7))), ranked AS ({RANKED}), selected AS(SELECT * FROM ranked WHERE ($5<>'cost' OR cost=(SELECT min(cost) FROM ranked) OR NOT EXISTS(SELECT 1 FROM ranked WHERE cost IS NOT NULL)) AND ($5<>'load' OR measured=(SELECT min(measured) FROM ranked))) SELECT ordinal,weight,child,leaf,path,identity,cost::text,loads FROM selected WHERE ($3::bigint IS NULL OR ordinal>$3) ORDER BY ordinal LIMIT $4")))
        .bind(choice).bind(parent).bind(after).bind(limit).bind(ranking.mode).bind(ranking.measure).bind(ranking.ceiling).fetch_all(conn).await?)
}
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PolicyNode {
    pub pool_id: Id,
    pub policy: Json,
    pub revision: i64,
    pub definition: Json,
    pub defaults: Json,
    pub proposal: Option<i64>,
}
pub async fn policy(conn: &mut Conn, choice: Id, id: Id) -> Result<PolicyNode, DbError> {
    Ok(sqlx::query_as("SELECT pool_id,policy,revision,definition,defaults,proposal FROM pool_choice_nodes WHERE choice_id=$1 AND id=$2").bind(choice).bind(id).fetch_one(conn).await?)
}
/// Relevant pool locks use one canonical order; no execution or external IO belongs under them.
pub async fn lock_policies(conn: &mut Conn, choice: Id) -> Result<(), DbError> {
    sqlx::query("SELECT p.id FROM pools p WHERE p.id IN(SELECT pool_id FROM pool_choice_nodes WHERE choice_id=$1) ORDER BY p.id FOR UPDATE").bind(choice).fetch_all(conn).await?;
    Ok(())
}
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Rotation {
    pub id: Id,
    pub weight: String,
    pub current_weight: String,
}
pub async fn rotation(
    conn: &mut Conn,
    pool: Id,
    identity: &Json,
    weight: &str,
    now: DateTime<Utc>,
) -> Result<Rotation, DbError> {
    let row:Option<Rotation>=sqlx::query_as("SELECT id,weight,current_weight FROM pool_rotation WHERE pool_id=$1 AND identity_hash=md5($2::jsonb::text) AND identity=$2 FOR UPDATE").bind(pool).bind(identity).fetch_optional(&mut *conn).await?;
    if let Some(row) = row {
        return Ok(row);
    }
    let id = orch_types::new_id();
    sqlx::query("INSERT INTO pool_rotation(id,pool_id,identity,weight,current_weight,updated_at) VALUES($1,$2,$3,$4,'0',$5)").bind(id).bind(pool).bind(identity).bind(weight).bind(now).execute(conn).await?;
    Ok(Rotation {
        id,
        weight: weight.into(),
        current_weight: "0".into(),
    })
}
pub async fn rotate(
    conn: &mut Conn,
    id: Id,
    weight: &str,
    current: &str,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE pool_rotation SET weight=$2,current_weight=$3,updated_at=$4 WHERE id=$1")
        .bind(id)
        .bind(weight)
        .bind(current)
        .bind(now)
        .execute(conn)
        .await?;
    Ok(())
}
pub async fn reset_ineligible(
    conn: &mut Conn,
    choice: Id,
    parent: Id,
    pool: Id,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(sqlx::AssertSqlSafe(format!("WITH edges AS ({EDGES}),tier AS(SELECT min(priority) priority FROM edges) UPDATE pool_rotation r SET current_weight='0',updated_at=$4 WHERE pool_id=$3 AND NOT EXISTS(SELECT 1 FROM edges e WHERE e.priority=(SELECT priority FROM tier) AND md5(e.identity::text)=r.identity_hash AND e.identity=r.identity)")))
        .bind(choice).bind(parent).bind(pool).bind(now).execute(conn).await?;
    Ok(())
}
pub async fn finish(
    conn: &mut Conn,
    id: Id,
    leaf: Option<&Json>,
    path: &Json,
    reason: Option<&str>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE pool_choices SET state=CASE WHEN $2::jsonb IS NULL THEN 'exhausted' ELSE 'selected' END,leaf=$2,path=$3,reason=$4,updated_at=$5 WHERE id=$1 AND state='building'").bind(id).bind(leaf).bind(path).bind(reason).bind(now).execute(conn).await?;
    Ok(())
}
pub async fn skipped(
    conn: &mut Conn,
    choice: Id,
    ordinal: i64,
    path: &Json,
    reason: &str,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO pool_choice_skips(choice_id,ordinal,path,reason) VALUES($1,$2,$3,$4)")
        .bind(choice)
        .bind(ordinal)
        .bind(path)
        .bind(reason)
        .execute(conn)
        .await?;
    Ok(())
}
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ObservedNode {
    pub id: Id,
    pub pool_id: Id,
    pub ordinal: i64,
    pub definition: Json,
    pub defaults: Json,
}
pub async fn observed(
    conn: &mut Conn,
    choice: Id,
    after: Option<i64>,
    limit: i64,
) -> Result<Vec<ObservedNode>, DbError> {
    Ok(sqlx::query_as("SELECT id,pool_id,ordinal,definition,defaults FROM pool_choice_nodes WHERE choice_id=$1 AND ($2::bigint IS NULL OR ordinal>$2) ORDER BY ordinal LIMIT $3").bind(choice).bind(after).bind(limit).fetch_all(conn).await?)
}

pub async fn observed_reverse(
    conn: &mut Conn,
    choice: Id,
    after: Option<i64>,
    limit: i64,
) -> Result<Vec<ObservedNode>, DbError> {
    Ok(sqlx::query_as("SELECT id,pool_id,ordinal,definition,defaults FROM pool_choice_nodes WHERE choice_id=$1 AND ($2::bigint IS NULL OR ordinal<$2) ORDER BY ordinal DESC LIMIT $3").bind(choice).bind(after).bind(limit).fetch_all(conn).await?)
}
pub async fn propose(
    conn: &mut Conn,
    choice: Id,
    node: Id,
    edge: Option<&Edge>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE pool_choice_nodes SET proposal=$3,cost=$4::text::numeric,loads=$5,feasible=$6 WHERE choice_id=$1 AND id=$2").bind(choice).bind(node).bind(edge.map(|edge|edge.ordinal)).bind(edge.and_then(|edge|edge.cost.as_deref())).bind(edge.map_or_else(||serde_json::json!({}),|edge|edge.loads.clone())).bind(edge.is_some()).execute(conn).await?;
    Ok(())
}
pub async fn edge(
    conn: &mut Conn,
    choice: Id,
    parent: Id,
    ordinal: i64,
) -> Result<Option<Edge>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("WITH edges AS ({EDGES}) SELECT ordinal,weight,child,leaf,path,identity,cost::text,loads FROM edges WHERE ordinal=$3"))).bind(choice).bind(parent).bind(ordinal).fetch_optional(conn).await?)
}
