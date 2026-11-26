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
fn load_scores(
    at: orch_types::Time,
    cpu: Option<(f64, i64)>,
    memory: Option<(i64, i64)>,
    pids: i64,
) -> serde_json::Map<String, Json> {
    let cpu = cpu
        .filter(|(_, limit)| *limit > 0)
        .and_then(|(percent, limit)| valid(percent * 1000.0 / limit as f64));
    let memory = memory
        .filter(|(_, limit)| *limit > 0)
        .and_then(|(bytes, limit)| valid(bytes as f64 * 100.0 / limit as f64));
    let mut loads = serde_json::Map::new();
    put(&mut loads, "cpu", cpu, at);
    put(&mut loads, "memory", memory, at);
    put(
        &mut loads,
        "combined",
        cpu.zip(memory).map(|(cpu, memory)| cpu.max(memory)),
        at,
    );
    put(
        &mut loads,
        "processes",
        (pids >= 0).then_some(pids as f64),
        at,
    );
    loads
}

/// Raw native observations use their actual denominator and keep counts as counts.
pub fn measured_environment_load(
    sample: &orch_driver_contract::units::ResourceSample,
    threads: Option<i64>,
) -> Json {
    let mut loads = load_scores(
        sample.at,
        sample
            .cpu_limit_millis
            .map(|limit| (sample.cpu_percent, limit)),
        sample
            .memory_limit_bytes
            .map(|limit| (sample.memory_bytes, limit)),
        sample.pids,
    );
    put(
        &mut loads,
        "threads",
        threads.map(|count| count as f64),
        sample.at,
    );
    Json::Object(loads)
}

/// Loads come from native observations and the engine's activity records.
pub(super) async fn native_loads(
    conn: &mut orch_db::Conn,
    leaf: &orch_provider_contract::pools::Leaf,
    now: orch_types::Time,
) -> Result<Json, crate::EngineError> {
    use orch_provider_contract::pools::Leaf;
    if let Leaf::Environment { id } = leaf {
        let Some(activity) = orch_db::queries::environments::current_load(conn, *id, now).await?
        else {
            return Ok(serde_json::json!({}));
        };
        let mut loads = activity
            .sample
            .as_ref()
            .map(|sample| {
                load_scores(
                    sample.at,
                    sample
                        .cpu_limit_millis
                        .filter(|_| activity.cpu_interval_measured)
                        .map(|limit| (sample.cpu_percent, limit)),
                    sample
                        .memory_limit_bytes
                        .map(|limit| (sample.memory_bytes, limit)),
                    sample.pids,
                )
            })
            .unwrap_or_default();
        put(&mut loads, "threads", Some(activity.threads as f64), now);
        return Ok(Json::Object(loads));
    }
    let Leaf::Host { id } = leaf else {
        return Ok(serde_json::json!({}));
    };
    let mut loads = serde_json::Map::new();
    let counts = orch_db::queries::placement_room::running_threads(conn, &[*id]).await?;
    put(
        &mut loads,
        "threads",
        counts.get(id).map(|count| *count as f64),
        now,
    );
    let Some(sample) = orch_db::queries::placement_room::host_samples(conn, &[*id])
        .await?
        .pop()
    else {
        return Ok(Json::Object(loads));
    };
    let capacity = sample.capacity;
    let load = sample.load;
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
    let observed = orch_types::Time::new(sample.load_at);
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
    Ok(Json::Object(loads))
}
