//! Durable port program options, wider admissions and their request fences.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_api_contract::ports::{PortHttpScheme, PortProgram};
use orch_types::{Id, Json};

/// The persistent fields of an opening, without any connection or traffic record.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Opening {
    /// Its stable reference.
    pub id: Id,
    /// Its owning environment.
    pub environment_id: Id,
    /// Its target port.
    pub port: i32,
    /// Its audience kind.
    pub kind: String,
    /// Its selected transports.
    pub transports: Vec<String>,
    /// Its selected entry methods.
    pub entry_methods: Vec<String>,
    /// Its named audience, cleared after removal.
    pub place_id: Option<Id>,
    /// Its public label.
    pub label: String,
    /// Its requesting person.
    pub requested_by: Option<Id>,
    /// Its requesting thread, cleared after purge.
    pub requested_by_thread: Option<Id>,
    /// Its current deciding person.
    pub allowed_by: Option<Id>,
    /// The immutable capability grant or policy that admitted it.
    pub admission: Option<Json>,
    /// When admission began.
    pub opened_at: DateTime<Utc>,
    /// Its bounded lapse.
    pub until_at: Option<DateTime<Utc>>,
    /// Admission and its current availability.
    pub state: String,
    /// A safe state explanation.
    pub reason: Option<String>,
    /// A link proof's digest, never projected publicly.
    pub secret_hash: Option<Vec<u8>>,
    /// Sealed only until intended disclosure is fenced.
    pub secret_sealed: Option<Vec<u8>>,
    /// The explicit intended link recipient.
    pub link_recipient: Option<String>,
    /// The intended person, when applicable.
    pub link_recipient_user: Option<Id>,
    /// The separate port decision, when applicable.
    pub approval_id: Option<Id>,
    /// The current host allocation owner.
    pub host_id: Option<Id>,
    /// The physical machine binding this public number, with or without a host facade.
    pub publish_machine_id: Option<Id>,
    /// The allocated or preferred host number.
    pub host_port: Option<i32>,
    /// Whether that preferred number is fixed.
    pub fixed_host_port: bool,
    /// Whether the host still owns the number through shutdown.
    pub host_allocated: bool,
    /// The authority revision.
    pub revision: i64,
    /// Its irreversible ending time.
    pub closed_at: Option<DateTime<Utc>>,
}

/// Locks an environment before its ports, serializing the distinct-port bound.
pub async fn lock_environment(conn: &mut Conn, environment: Id) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar::<_, Id>("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(environment)
            .fetch_optional(conn)
            .await?
            .is_some(),
    )
}

/// Locks one target without materializing a pure read's revision-zero row.
pub async fn lock_target(conn: &mut Conn, environment: Id, port: u16) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("port:{environment}:{port}"))
        .execute(conn)
        .await?;
    Ok(())
}

/// Reads the target's one program and revision, or its unwritten defaults.
pub async fn program(
    conn: &mut Conn,
    environment: Id,
    port: u16,
) -> Result<(PortProgram, i64), DbError> {
    let row: Option<(String, Option<String>, bool, i64)> = sqlx::query_as(
        "SELECT http_scheme,host_header,proxy_protocol,revision FROM port_targets WHERE environment_id=$1 AND port=$2")
        .bind(environment).bind(i32::from(port)).fetch_optional(conn).await?;
    Ok(row.map_or(
        (PortProgram::default(), 0),
        |(scheme, host_header, proxy_protocol, revision)| {
            (
                PortProgram {
                    http_scheme: if scheme == "https" {
                        PortHttpScheme::Https
                    } else {
                        PortHttpScheme::Http
                    },
                    host_header,
                    proxy_protocol,
                },
                revision,
            )
        },
    ))
}

/// Applies a complete program change under the caller's target lock and revision.
pub async fn set_program(
    conn: &mut Conn,
    environment: Id,
    port: u16,
    program: &PortProgram,
    expected: i64,
) -> Result<bool, DbError> {
    let scheme = if program.http_scheme == PortHttpScheme::Https {
        "https"
    } else {
        "http"
    };
    let changed = if expected == 0 {
        sqlx::query("INSERT INTO port_targets(environment_id,port,http_scheme,host_header,proxy_protocol) VALUES($1,$2,$3,$4,$5) ON CONFLICT DO NOTHING")
            .bind(environment).bind(i32::from(port)).bind(scheme).bind(&program.host_header).bind(program.proxy_protocol).execute(conn).await?.rows_affected()
    } else {
        sqlx::query("UPDATE port_targets SET http_scheme=$3,host_header=$4,proxy_protocol=$5,revision=revision+1 WHERE environment_id=$1 AND port=$2 AND revision=$6")
            .bind(environment).bind(i32::from(port)).bind(scheme).bind(&program.host_header).bind(program.proxy_protocol).bind(expected).execute(conn).await?.rows_affected()
    };
    Ok(changed == 1)
}

/// Reads one opening's durable authority and safe metadata.
pub async fn opening(conn: &mut Conn, id: Id) -> Result<Option<Opening>, DbError> {
    Ok(sqlx::query_as("SELECT * FROM port_openings WHERE id=$1")
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// Lists an environment's admissions, including pending and unavailable ones.
pub async fn openings(conn: &mut Conn, environment: Id) -> Result<Vec<Opening>, DbError> {
    Ok(
        sqlx::query_as("SELECT * FROM port_openings WHERE environment_id=$1 ORDER BY opened_at,id")
            .bind(environment)
            .fetch_all(conn)
            .await?,
    )
}

/// Reads only effective wider rows for a batch of environment reference signs.
pub async fn openings_among(conn: &mut Conn, environments: &[Id]) -> Result<Vec<Opening>, DbError> {
    Ok(sqlx::query_as("SELECT * FROM port_openings WHERE environment_id=ANY($1) AND state NOT IN ('closed','refused','waiting') ORDER BY environment_id,opened_at,id").bind(environments).fetch_all(conn).await?)
}

/// Counts distinct wider ports, counting a pending decision once per number.
pub async fn used(conn: &mut Conn, environment: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar("SELECT count(DISTINCT port) FROM port_openings WHERE environment_id=$1 AND state NOT IN ('closed','refused')")
        .bind(environment).fetch_one(conn).await?)
}

/// Inserts the admission chosen by a current guarded producer.
pub async fn insert(conn: &mut Conn, row: &Opening) -> Result<(), DbError> {
    sqlx::query("INSERT INTO port_openings(id,environment_id,port,kind,transports,entry_methods,place_id,label,requested_by,requested_by_thread,allowed_by,admission,opened_at,until_at,state,reason,secret_hash,secret_sealed,link_recipient,link_recipient_user,approval_id,host_id,host_port,fixed_host_port,host_allocated,revision,closed_at,publish_machine_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27,$28)")
        .bind(row.id).bind(row.environment_id).bind(row.port).bind(&row.kind).bind(&row.transports).bind(&row.entry_methods).bind(row.place_id).bind(&row.label)
        .bind(row.requested_by).bind(row.requested_by_thread).bind(row.allowed_by).bind(&row.admission).bind(row.opened_at).bind(row.until_at).bind(&row.state).bind(&row.reason)
        .bind(&row.secret_hash).bind(&row.secret_sealed).bind(&row.link_recipient).bind(row.link_recipient_user).bind(row.approval_id).bind(row.host_id).bind(row.host_port)
        .bind(row.fixed_host_port).bind(row.host_allocated).bind(row.revision).bind(row.closed_at).bind(row.publish_machine_id).execute(conn).await?;
    Ok(())
}

/// Admits an existing waiting row once; the approval and delivery owner shares this transaction.
pub async fn grant(conn: &mut Conn, row: &Opening, expected: i64) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE port_openings SET allowed_by=$2,admission=$3,state=$4,reason=NULL,secret_hash=$5,secret_sealed=$6,link_recipient=$7,link_recipient_user=$8,host_id=$9,host_port=$10,host_allocated=$11,publish_machine_id=$13,revision=revision+1 WHERE id=$1 AND revision=$12 AND state='waiting'")
        .bind(row.id).bind(row.allowed_by).bind(&row.admission).bind(&row.state).bind(&row.secret_hash).bind(&row.secret_sealed).bind(&row.link_recipient).bind(row.link_recipient_user)
        .bind(row.host_id).bind(row.host_port).bind(row.host_allocated).bind(expected).bind(row.publish_machine_id).execute(conn).await?.rows_affected()==1)
}

/// Ends admission irreversibly, keeping a host allocation until shutdown acknowledgement.
pub async fn close(
    conn: &mut Conn,
    id: Id,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE port_openings SET state='closed',reason=$2,closed_at=$3,secret_hash=NULL,secret_sealed=NULL,revision=revision+1 WHERE id=$1 AND state<>'closed'")
        .bind(id).bind(reason).bind(now).execute(conn).await?.rows_affected()==1)
}

/// Updates runtime availability without changing admission's authority revision.
pub async fn availability(
    conn: &mut Conn,
    id: Id,
    state: &str,
    reason: Option<&str>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE port_openings SET state=$2,reason=$3 WHERE id=$1 AND state NOT IN ('waiting','refused','closed') AND (state IS DISTINCT FROM $2 OR reason IS DISTINCT FROM $3)")
        .bind(id).bind(state).bind(reason).execute(conn).await?.rows_affected()==1)
}

/// Confirms that a host has ended its actual listener and connections before releasing the number.
pub async fn release_host(conn: &mut Conn, id: Id, revision: i64) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE port_openings SET host_allocated=false WHERE id=$1 AND revision=$2 AND host_allocated")
        .bind(id).bind(revision).execute(conn).await?.rows_affected()==1)
}

/// A person's or thread's full-body and resolved-effect fence.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Request {
    /// The selected environment.
    pub environment_id: Id,
    /// The person, mutually exclusive with thread.
    pub user_id: Option<Id>,
    /// The thread, mutually exclusive with person.
    pub thread_id: Option<Id>,
    /// The client reference.
    pub client_id: Id,
    /// Complete canonical body digest.
    pub request_hash: Vec<u8>,
    /// Resolved effect digest, without reason.
    pub effect_hash: Vec<u8>,
    /// The one resulting opening.
    pub opening_id: Id,
    /// The refusal's replay bound.
    pub refused_until: Option<DateTime<Utc>>,
    /// Whether the intended once-secret response was emitted.
    pub secret_response_taken: bool,
    /// Whether the initial outcome was delivered.
    pub outcome_delivered: bool,
    /// When this reference was recorded.
    pub created_at: DateTime<Utc>,
}

/// Reads an exact principal and client fence under the environment lock.
pub async fn request(
    conn: &mut Conn,
    environment: Id,
    user: Option<Id>,
    thread: Option<Id>,
    client: Id,
) -> Result<Option<Request>, DbError> {
    Ok(sqlx::query_as("SELECT * FROM port_open_requests WHERE environment_id=$1 AND user_id IS NOT DISTINCT FROM $2 AND thread_id IS NOT DISTINCT FROM $3 AND client_id=$4")
        .bind(environment).bind(user).bind(thread).bind(client).fetch_optional(conn).await?)
}

/// Reads the same principal's current effect, including a bounded refusal replay.
pub async fn effect(
    conn: &mut Conn,
    environment: Id,
    user: Option<Id>,
    thread: Option<Id>,
    hash: &[u8],
    now: DateTime<Utc>,
) -> Result<Option<Request>, DbError> {
    Ok(sqlx::query_as("SELECT r.* FROM port_open_requests r JOIN port_openings o ON o.id=r.opening_id WHERE r.environment_id=$1 AND r.user_id IS NOT DISTINCT FROM $2 AND r.thread_id IS NOT DISTINCT FROM $3 AND r.effect_hash=$4 AND (o.state NOT IN ('closed','refused') OR r.refused_until>$5) ORDER BY r.created_at LIMIT 1")
        .bind(environment).bind(user).bind(thread).bind(hash).bind(now).fetch_optional(conn).await?)
}

/// Adds a client fence to the one admission effect, without a second audience.
pub async fn save_request(conn: &mut Conn, row: &Request) -> Result<(), DbError> {
    sqlx::query("INSERT INTO port_open_requests(environment_id,user_id,thread_id,client_id,request_hash,effect_hash,opening_id,refused_until,secret_response_taken,outcome_delivered,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
        .bind(row.environment_id).bind(row.user_id).bind(row.thread_id).bind(row.client_id).bind(&row.request_hash).bind(&row.effect_hash).bind(row.opening_id).bind(row.refused_until)
        .bind(row.secret_response_taken).bind(row.outcome_delivered).bind(row.created_at).execute(conn).await?;
    Ok(())
}

/// Sets a new link lapse without reviving an ended admission.
pub async fn lapse(
    conn: &mut Conn,
    id: Id,
    expected: i64,
    until: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE port_openings SET until_at=$3,revision=revision+1 WHERE id=$1 AND revision=$2 AND kind='link' AND state NOT IN ('closed','refused')")
        .bind(id).bind(expected).bind(until).execute(conn).await?.rows_affected()==1)
}

/// Fences the first accepted link use in a UTC day across every entry method.
pub async fn link_used(conn: &mut Conn, id: Id, now: DateTime<Utc>) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "INSERT INTO port_link_uses(opening_id,day) VALUES($1,$2) ON CONFLICT DO NOTHING",
    )
    .bind(id)
    .bind(now.date_naive())
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// Fences an intended once response and erases its recoverable sealed proof in the same transaction.
pub async fn delivered(conn: &mut Conn, id: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE port_open_requests SET secret_response_taken=true WHERE opening_id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("UPDATE port_openings SET secret_sealed=NULL WHERE id=$1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(())
}

/// A host's machine, its saved declaration of host ports and that declaration's acknowledged revision.
pub async fn host_declaration(
    conn: &mut Conn,
    host: Id,
) -> Result<Option<(Id, Option<serde_json::Value>, i64)>, DbError> {
    Ok(super::environment_facts::host_ports(conn, &[host])
        .await?
        .pop()
        .map(|row| (row.machine_id, row.publish, row.revision)))
}

/// The physical publisher's declaration and policy revision, from the same facts on every role.
pub async fn publisher_declaration(
    conn: &mut Conn,
    machine: Id,
) -> Result<Option<(Id, Option<Json>, i64)>, DbError> {
    Ok(super::environment_facts::publisher(conn, machine)
        .await?
        .map(|publisher| {
            let ports = publisher.ports.and_then(|ports| {
                serde_json::from_value::<orch_link_contract::policy::MachinePorts>(ports).ok()
            });
            let revision = ports
                .as_ref()
                .map_or(0, orch_link_contract::policy::MachinePorts::revision);
            let publish = ports.and_then(|ports| {
                if ports.permits(orch_api_contract::ports::PortOpeningKind::Host) {
                    ports.publish
                } else {
                    None
                }
            });
            (
                publisher.machine_id,
                publish.and_then(|publish| serde_json::to_value(publish).ok()),
                revision,
            )
        }))
}

/// A host's name and saved declaration of host ports, for the view of an opening on it.
pub async fn host_of_opening(
    conn: &mut Conn,
    host: Id,
) -> Result<Option<(String, Option<serde_json::Value>)>, DbError> {
    Ok(super::environment_facts::host_ports(conn, &[host])
        .await?
        .pop()
        .map(|row| (row.name, row.publish)))
}

/// Records what a host's node answered to a publication of `opening` at `revision`: bound (the opening is
/// served from its number), or not (the number is free again, and the opening unavailable with the reason).
/// A newer revision is left as it is. Returns whether the row was at that revision.
pub async fn host_published(
    conn: &mut Conn,
    opening: Id,
    revision: i64,
    bound: bool,
    reason: Option<&str>,
) -> Result<bool, DbError> {
    let changed = if bound {
        sqlx::query(
            "UPDATE port_openings SET host_allocated = true, reason = NULL,
                    state = CASE WHEN state IN ('opening','unavailable') THEN 'open' ELSE state END
              WHERE id = $1 AND revision = $2 AND state NOT IN ('closed','refused')",
        )
        .bind(opening)
        .bind(revision)
        .execute(conn)
        .await?
    } else {
        sqlx::query(
            "UPDATE port_openings SET host_allocated = false, reason = $3, state = 'unavailable'
              WHERE id = $1 AND revision = $2 AND state NOT IN ('closed','refused')",
        )
        .bind(opening)
        .bind(revision)
        .bind(reason)
        .execute(conn)
        .await?
    };
    Ok(changed.rows_affected() > 0)
}

/// The openings on `host` that stand with their number held, with their revisions.
pub async fn host_openings_on(conn: &mut Conn, host: Id) -> Result<Vec<(Id, i64)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, revision FROM port_openings
          WHERE host_id = $1 AND host_allocated AND state NOT IN ('closed','refused') ORDER BY id",
    )
    .bind(host)
    .fetch_all(conn)
    .await?)
}

/// The host openings that stand with their number held, each with the machine of its host's node, which is
/// told them again from time to time and whenever it comes back.
pub async fn host_openings_held(conn: &mut Conn) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.id, o.publish_machine_id FROM port_openings o JOIN machines m ON m.id = o.publish_machine_id
          WHERE o.kind = 'host' AND o.host_allocated AND o.state NOT IN ('closed','refused')
          ORDER BY o.id",
    )
    .fetch_all(conn)
    .await?)
}

/// The host openings that stand unavailable with their number free (a node that could not bind it, or no longer
/// serves it), each with the machine of its host's node, which is asked to serve them again from time to time.
pub async fn host_openings_unserved(conn: &mut Conn) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.id, o.publish_machine_id FROM port_openings o JOIN machines m ON m.id = o.publish_machine_id
          WHERE o.kind = 'host' AND NOT o.host_allocated AND o.state = 'unavailable'
            AND o.host_port IS NOT NULL AND jsonb_typeof(m.facts#>'{policy,ports,publish}')='object'
          ORDER BY o.id",
    )
    .fetch_all(conn)
    .await?)
}

/// Reserves the number of an unavailable host opening again, where no other opening holds it now. Returns
/// whether it is reserved.
pub async fn host_reserve_again(conn: &mut Conn, opening: Id) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE port_openings o SET host_allocated = true
          WHERE o.id = $1 AND o.kind = 'host' AND NOT o.host_allocated AND o.state = 'unavailable'
            AND o.host_port IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM port_openings p
                             WHERE p.publish_machine_id = o.publish_machine_id AND p.host_port = o.host_port
                               AND p.host_allocated AND p.id <> o.id)",
    )
    .bind(opening)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Frees the number reserved again for an unavailable host opening whose node could not be asked.
pub async fn host_unreserve(conn: &mut Conn, opening: Id) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE port_openings SET host_allocated = false
          WHERE id = $1 AND kind = 'host' AND state = 'unavailable'",
    )
    .bind(opening)
    .execute(conn)
    .await?;
    Ok(())
}

/// A standing host opening beside where its environment is now: what the lifecycle pass follows.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HostLife {
    /// The opening.
    pub id: Id,
    /// Its environment.
    pub environment_id: Id,
    /// The host it was reviewed and published on.
    pub host_id: Option<Id>,
    /// The machine it was reviewed and published on.
    pub publish_machine_id: Option<Id>,
    /// Its number: held, or the one it prefers while it holds none.
    pub host_port: Option<i32>,
    /// Whether its maker fixed that number.
    pub fixed_host_port: bool,
    /// Whether it holds that number now.
    pub host_allocated: bool,
    /// Its state.
    pub state: String,
    /// Its environment's status.
    pub status: String,
    /// The host its environment's unit is on now, if any.
    pub current_host: Option<Id>,
    /// The physical machine currently hosting this directory or unit.
    pub current_machine: Option<Id>,
}

/// Every standing host opening, with its environment's status and the host its unit is on now.
pub async fn host_lives(conn: &mut Conn) -> Result<Vec<HostLife>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.id, o.environment_id, o.host_id,o.publish_machine_id, o.host_port, o.fixed_host_port, o.host_allocated, o.state,
                e.status,r.host_id AS current_host,COALESCE(h.machine_id,e.machine_id) AS current_machine
           FROM port_openings o JOIN environments e ON e.id = o.environment_id
           LEFT JOIN LATERAL (SELECT r.host_id FROM runtimes r
               WHERE r.kind='environment' AND r.environment_id=e.id AND r.state NOT IN ('retired','removed') AND r.successor_of IS NULL
                 AND NOT (e.status='moving' AND e.machine_id IS NOT NULL AND r.generation>e.generation)
               ORDER BY r.created_at DESC LIMIT 1) r ON true
           LEFT JOIN hosts h ON h.id=r.host_id
          WHERE o.kind = 'host' AND o.state NOT IN ('closed', 'refused', 'waiting') AND e.deleted_at IS NULL
          ORDER BY o.id",
    )
    .fetch_all(conn)
    .await?)
}

/// Gives back the number a host opening holds, keeping it as the number it prefers, with the state and reason it
/// waits under: its environment stopped, or moved (`host` the host it is on now). Returns whether the row changed.
pub async fn host_release(
    conn: &mut Conn,
    opening: Id,
    state: &str,
    reason: &str,
    host: Option<Id>,
    machine: Option<Id>,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE port_openings SET host_allocated = false, state = $2, reason = $3, host_id = $4,publish_machine_id=$5
          WHERE id = $1 AND kind = 'host' AND state NOT IN ('closed', 'refused', 'waiting')",
    )
    .bind(opening)
    .bind(state)
    .bind(reason)
    .bind(host)
    .bind(machine)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Takes `number` on its host for a host opening that holds none, where no other opening holds it now; the
/// opening is opening until its node answers. Returns whether it took it.
pub async fn host_take(conn: &mut Conn, opening: Id, number: i32) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE port_openings o SET host_allocated = true, host_port = $2, state = 'opening', reason = NULL
          WHERE o.id = $1 AND o.kind = 'host' AND NOT o.host_allocated AND o.state IN ('stopped', 'unavailable')
            AND NOT EXISTS (SELECT 1 FROM port_openings p
                             WHERE p.publish_machine_id = o.publish_machine_id AND p.host_port = $2 AND p.host_allocated AND p.id <> o.id)",
    )
    .bind(opening)
    .bind(number)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Says why a host opening that holds no number waits: its fixed number is held or lies outside the ranges.
pub async fn host_waits(conn: &mut Conn, opening: Id, reason: &str) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE port_openings SET state = 'unavailable', reason = $2
          WHERE id = $1 AND kind = 'host' AND NOT host_allocated AND state IN ('stopped', 'unavailable')",
    )
    .bind(opening)
    .bind(reason)
    .execute(conn)
    .await?;
    Ok(())
}

/// Nodes which can still own this environment's active or recent host connection record.
pub async fn record_nodes(
    conn: &mut Conn,
    environment: Id,
    since: DateTime<Utc>,
    limit: u32,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT DISTINCT o.publish_machine_id FROM port_openings o JOIN machines m ON m.id=o.publish_machine_id
        WHERE o.environment_id=$1 AND o.kind='host' AND o.state NOT IN ('waiting','refused')
        AND (o.closed_at IS NULL OR o.closed_at >= $2) ORDER BY o.publish_machine_id LIMIT $3")
        .bind(environment).bind(since).bind(i64::from(limit)).fetch_all(conn).await?)
}

/// The host numbers the openings on `host` hold now: reserved, bound or retiring, for either transport.
pub async fn host_ports_held(conn: &mut Conn, host: Id) -> Result<Vec<i32>, DbError> {
    let machine: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
        .bind(host)
        .fetch_optional(&mut *conn)
        .await?;
    match machine {
        Some(machine) => publisher_ports_held(conn, machine).await,
        None => Ok(Vec::new()),
    }
}

/// Numbers held by the physical machine across all its directories and units.
pub async fn publisher_ports_held(conn: &mut Conn, machine: Id) -> Result<Vec<i32>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT host_port FROM port_openings
          WHERE publish_machine_id = $1 AND host_allocated AND host_port IS NOT NULL ORDER BY host_port",
    )
    .bind(machine)
    .fetch_all(conn)
    .await?)
}

/// Reads environments with effective outside admissions for reconciliation.
pub async fn admitted_environments(conn: &mut Conn) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT DISTINCT environment_id FROM port_openings WHERE state NOT IN ('closed','refused') ORDER BY environment_id")
        .fetch_all(conn).await?)
}

/// Reads safe outside admissions homed at or below a named place, including stopped ones.
pub async fn outside(
    conn: &mut Conn,
    node: Id,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<Opening>, DbError> {
    Ok(sqlx::query_as("SELECT o.* FROM port_openings o JOIN environments e ON e.id=o.environment_id JOIN nodes h ON h.id=e.home_node_id JOIN nodes scope ON scope.id=$1 WHERE scope.id=ANY(h.path) AND o.state NOT IN ('closed','refused','waiting') AND ($2::uuid IS NULL OR o.id>$2) ORDER BY o.id LIMIT $3")
        .bind(node).bind(after).bind(limit).fetch_all(conn).await?)
}

/// Records owned traffic activity at the existing watch cadence, with no traffic row.
pub async fn activity(
    conn: &mut Conn,
    environment: Id,
    generation: i64,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE environments SET last_activity_at=GREATEST(last_activity_at,$3) WHERE id=$1 AND generation=$2 AND status='running' AND deleted_at IS NULL")
        .bind(environment).bind(generation).bind(at).execute(conn).await?;
    Ok(())
}

/// Reads current machine limits without inferring a grant from an absent key.
pub async fn machine_policy(conn: &mut Conn, machine: Id) -> Result<Json, DbError> {
    Ok(
        sqlx::query_scalar(
            "SELECT COALESCE(facts->'policy','{}'::jsonb) FROM machines WHERE id=$1",
        )
        .bind(machine)
        .fetch_one(conn)
        .await?,
    )
}

/// Whether the separately generated admission schema is installed on this database.
pub async fn installed(conn: &mut Conn) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT to_regclass('port_openings') IS NOT NULL")
            .fetch_one(conn)
            .await?,
    )
}

/// Records the card of a thread's port request: its waiting opening's decision, which whoever holds
/// Ports Manage on the environment now may take, until `expires_at`.
#[expect(
    clippy::too_many_arguments,
    reason = "each value is one column of the card that the decision reads back"
)]
pub async fn insert_approval(
    conn: &mut Conn,
    id: Id,
    thread: Id,
    environment: Id,
    title: &str,
    request: &Json,
    expires_at: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO approvals (id, kind, state, node_id, thread_id, environment_id, requester_thread_id, title,
                                request, required_permission, expires_at, created_at, updated_at)
         VALUES ($1, 'port', 'pending', $2, $2, $3, $2, $4, $5, 'ports.manage', $6, $7, $7)",
    )
    .bind(id)
    .bind(thread)
    .bind(environment)
    .bind(title)
    .bind(request)
    .bind(expires_at)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Settles a port card once: `approved`, `denied`, `cancelled` or `expired`, by `by` (none for the
/// system), with what was decided. False when it was settled already.
pub async fn settle_approval(
    conn: &mut Conn,
    id: Id,
    state: &str,
    by: Option<Id>,
    decision: &Json,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    if !super::approvals::approval(conn, id)
        .await?
        .is_some_and(|row| row.kind == "port")
    {
        return Ok(false);
    }
    super::approvals::decide(conn, id, state, by, decision, at).await
}

/// Refuses a waiting opening: it is ended, and its requests refuse the same effect again until
/// `replay_until` without another card.
pub async fn refuse(
    conn: &mut Conn,
    id: Id,
    reason: &str,
    replay_until: DateTime<Utc>,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let refused = sqlx::query(
        "UPDATE port_openings SET state='refused', reason=$2, closed_at=$3, revision=revision+1
          WHERE id=$1 AND state='waiting'",
    )
    .bind(id)
    .bind(reason)
    .bind(at)
    .execute(&mut *conn)
    .await?
    .rows_affected()
        == 1;
    if refused {
        sqlx::query("UPDATE port_open_requests SET refused_until=$2 WHERE opening_id=$1")
            .bind(id)
            .bind(replay_until)
            .execute(conn)
            .await?;
    }
    Ok(refused)
}

/// The waiting openings a thread requested whose card ran out, with their cards.
pub async fn lapsed_waiting(conn: &mut Conn, now: DateTime<Utc>) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT o.id, o.approval_id FROM port_openings o JOIN approvals a ON a.id = o.approval_id
          WHERE o.state = 'waiting' AND a.state = 'pending' AND a.expires_at <= $1",
    )
    .bind(now)
    .fetch_all(conn)
    .await?)
}

/// Marks that the first outcome of a request was told to its thread, so that it is told once.
pub async fn outcome_told(conn: &mut Conn, opening: Id) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE port_open_requests SET outcome_delivered = true
          WHERE opening_id = $1 AND thread_id IS NOT NULL AND NOT outcome_delivered",
    )
    .bind(opening)
    .execute(conn)
    .await?
    .rows_affected()
        > 0)
}

/// Writes what a port card shows, once its waiting opening exists.
pub async fn set_card(conn: &mut Conn, approval: Id, card: &Json) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE approvals SET request = $2, updated_at = now() WHERE id = $1 AND kind = 'port'",
    )
    .bind(approval)
    .bind(card)
    .execute(conn)
    .await?;
    Ok(())
}

/// Who gave an environment the capability `key` the last time it was given, and when: the newest
/// `capability.granted` of that environment for that key. A grant is the environment's and keeps no
/// author of its own; the trail says whose decision it was (none where a policy gave it).
pub async fn granted(
    conn: &mut Conn,
    environment: Id,
    key: &str,
) -> Result<Option<(Option<Id>, DateTime<Utc>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT CASE WHEN actor_kind = 'user' THEN actor_id END, at FROM audit_log
          WHERE environment_id = $1 AND action = 'capability.granted' AND details->>'capability' = $2
          ORDER BY at DESC LIMIT 1",
    )
    .bind(environment)
    .bind(key)
    .fetch_optional(conn)
    .await?)
}

/// The opening a port card decides.
pub async fn opening_of_approval(
    conn: &mut Conn,
    approval: Id,
) -> Result<Option<Opening>, DbError> {
    Ok(
        sqlx::query_as("SELECT * FROM port_openings WHERE approval_id = $1")
            .bind(approval)
            .fetch_optional(conn)
            .await?,
    )
}
