//! Native request measurements, separate from eligibility and policy. Missing is never zero.
use crate::rates::Priced;
use orch_types::Json;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Quote {
    pub amount: String,
    pub currency: String,
    pub estimated: bool,
    pub basis: Json,
}
impl Quote {
    /// Uses the existing applied request-rate result, including tier/context/cache handling.
    pub fn from_priced(priced: Priced) -> Option<Self> {
        let amount = priced.credits?;
        let found = priced.found?;
        Some(Self {
            amount,
            currency: "credits".into(),
            estimated: true,
            basis: serde_json::json!({"source":found.source.word(),"rate":found.rate}),
        })
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Measurements {
    pub quote: Option<Quote>,
    pub loads: Json,
}

fn valid(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}
fn put(
    loads: &mut serde_json::Map<String, Json>,
    key: &str,
    score: Option<f64>,
    at: orch_types::Time,
) {
    if let Some(score) = score.and_then(valid) {
        loads.insert(key.into(), serde_json::json!({"score":score,"at":at}));
    }
}
/// Raw native observations use their actual denominator and keep counts as counts.
pub fn measured_environment_load(
    sample: &orch_driver_contract::units::ResourceSample,
    threads: Option<i64>,
) -> Json {
    let cpu = sample
        .cpu_limit_millis
        .filter(|value| *value > 0)
        .and_then(|limit| valid(sample.cpu_percent * 1000.0 / limit as f64));
    let memory = sample
        .memory_limit_bytes
        .filter(|value| *value > 0)
        .and_then(|limit| valid(sample.memory_bytes as f64 * 100.0 / limit as f64));
    let mut loads = serde_json::Map::new();
    put(&mut loads, "cpu", cpu, sample.at);
    put(&mut loads, "memory", memory, sample.at);
    put(
        &mut loads,
        "combined",
        cpu.zip(memory).map(|(cpu, memory)| cpu.max(memory)),
        sample.at,
    );
    put(
        &mut loads,
        "processes",
        (sample.pids >= 0).then_some(sample.pids as f64),
        sample.at,
    );
    put(
        &mut loads,
        "threads",
        threads
            .filter(|count| *count >= 0)
            .map(|count| count as f64),
        sample.at,
    );
    Json::Object(loads)
}
/// This reads only current native host telemetry and DB activity, with no source-directory IO.
pub(super) async fn native_loads(
    conn: &mut orch_db::Conn,
    leaf: &orch_provider_contract::pools::Leaf,
    now: orch_types::Time,
) -> Result<Json, crate::EngineError> {
    use orch_provider_contract::pools::Leaf;
    let Leaf::Host { id } = leaf else {
        return Ok(serde_json::json!({}));
    };
    let row:Option<(Json,Json,Option<chrono::DateTime<chrono::Utc>>,bool)>=sqlx::query_as("SELECT h.load,h.capacity,h.load_at,m.online FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=$1 AND m.deleted_at IS NULL").bind(id).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let Some((load, capacity, Some(at), true)) = row else {
        return Ok(serde_json::json!({}));
    };
    let cpu = load
        .get("cpu_percent")
        .and_then(Json::as_f64)
        .zip(
            capacity
                .get("cpu_millis")
                .and_then(Json::as_f64)
                .filter(|value| *value > 0.0)
                .or_else(|| {
                    capacity
                        .get("cpus")
                        .and_then(Json::as_f64)
                        .filter(|value| *value > 0.0)
                        .map(|value| value * 1000.0)
                }),
        )
        .and_then(|(used, limit)| valid(used * 1000.0 / limit));
    let memory = load
        .get("memory_used_mb")
        .and_then(Json::as_f64)
        .zip(
            capacity
                .get("memory_mb")
                .and_then(Json::as_i64)
                .filter(|value| *value > 0),
        )
        .and_then(|(used, limit)| valid(used * 100.0 / limit as f64));
    let mut loads = serde_json::Map::new();
    let observed = orch_types::Time::new(at);
    put(&mut loads, "cpu", cpu, observed);
    put(&mut loads, "memory", memory, observed);
    put(
        &mut loads,
        "combined",
        cpu.zip(memory).map(|(cpu, memory)| cpu.max(memory)),
        observed,
    );
    put(
        &mut loads,
        "processes",
        load.get("pids")
            .and_then(Json::as_i64)
            .filter(|count| *count >= 0)
            .map(|count| count as f64),
        observed,
    );
    let threads:i64=sqlx::query_scalar("SELECT count(*) FROM threads t JOIN nodes n ON n.id=t.node_id JOIN environments e ON e.id=t.environment_id WHERE t.state='running' AND n.deleted_at IS NULL AND e.deleted_at IS NULL AND (e.machine_id=(SELECT machine_id FROM hosts WHERE id=$1) OR EXISTS(SELECT 1 FROM runtimes r WHERE r.environment_id=e.id AND r.host_id=$1 AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed')))").bind(id).fetch_one(conn).await.map_err(orch_db::DbError::from)?;
    put(&mut loads, "threads", Some(threads as f64), now);
    Ok(Json::Object(loads))
}
