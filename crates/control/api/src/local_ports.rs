//! Read-only inside port projections and live peer assertions, scoped to the authenticated current environment machine.

use crate::{
    middleware::{
        authentication::Caller,
        requests::{ApiError, ApiResult},
    },
    registry::Ctx,
    routes::{port_entry, ports},
    state::App,
};
use orch_api_contract::{
    common::ErrorCode,
    ports::{
        EnvironmentPortConnectionListQuery, EnvironmentPortList, PortCaller, PortPeer,
        PortTransport,
    },
    routes::Endpoint,
};
use orch_authz::{Principal, TokenGrant};
use orch_db::{
    Conn,
    queries::{environments::routes::EnvironmentFull, ports as rows, resources},
};
use orch_link_contract::local_ports::{
    PENDING_PER_ENVIRONMENT, PortLocalAnswer, PortLocalAsk, PortLocalData, PortLocalFailure,
    PortLocalRead,
};
use orch_pki::port_assertions::{AssertedCaller, PortAssertion};
use orch_types::Id;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

/// Only currently pending environments occupy this per-environment admission map.
#[derive(Default)]
pub(crate) struct Pending(Arc<Mutex<HashMap<Id, usize>>>);
/// Cancellation releases this environment's slot and forgets its counter when it reaches zero.
pub(crate) struct Permit {
    pending: Arc<Mutex<HashMap<Id, usize>>>,
    environment: Id,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut held = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = held.get_mut(&self.environment) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                held.remove(&self.environment);
            }
        }
    }
}
impl Pending {
    /// Refuses excess reads instead of queueing more retained caller queries.
    pub(crate) fn enter(&self, environment: Id) -> Option<Permit> {
        let mut held = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let count = held.entry(environment).or_default();
        if *count >= PENDING_PER_ENVIRONMENT {
            return None;
        }
        *count += 1;
        Some(Permit {
            pending: Arc::clone(&self.0),
            environment,
        })
    }
}

async fn scope(conn: &mut Conn, machine: Id, ask: &PortLocalAsk) -> ApiResult<EnvironmentFull> {
    resources::machine(conn, machine)
        .await?
        .filter(|row| row.status == "active" && row.deleted_at.is_none())
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the inside port reader's machine is not active",
            )
        })?;
    let env = ports::environment(conn, ask.environment_id).await?;
    if env.machine_id != Some(machine) || i64::from(env.generation) != ask.generation {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the environment is not on this machine at this generation",
        ));
    }
    Ok(env)
}

/// The request's identity is supplied by the backplane, never by an untrusted body field.
pub(crate) async fn answer(app: &App, machine: Id, ask: PortLocalAsk) -> PortLocalAnswer {
    let request_id = ask.request_id;
    match read(app, machine, &ask).await {
        Ok(data) => PortLocalAnswer {
            request_id,
            data: Some(data),
            failure: None,
        },
        Err(error) => PortLocalAnswer {
            request_id,
            data: None,
            failure: Some(PortLocalFailure {
                code: error.code().as_str().into(),
                detail: error.problem.detail,
            }),
        },
    }
}

async fn read(app: &App, machine: Id, ask: &PortLocalAsk) -> ApiResult<PortLocalData> {
    if matches!(ask.read, PortLocalRead::Keys) {
        return Ok(PortLocalData::Keys {
            keys: port_entry::assertion_keys(app),
        });
    }
    let mut conn = app.db.acquire().await?;
    let mut env = scope(&mut conn, machine, ask).await?;
    let ctx = Ctx::for_tool(
        app.clone(),
        Caller::Anonymous,
        EnvironmentPortList::ROUTE,
        env.id,
    );
    if matches!(&ask.read,PortLocalRead::List {query} if query.look==Some(true)) {
        drop(conn);
        let _ = app.cache.ports().look(&ctx, &env).await;
        conn = app.db.acquire().await?;
        env = scope(&mut conn, machine, ask).await?;
    }
    match &ask.read {
        PortLocalRead::List { query } => {
            // This ordinary principal only redacts projected names. Admission is the exact
            // inside-machine scope, never a fabricated user token or administrator bypass.
            let principal = Principal::load(
                &mut conn,
                orch_authz::Caller::token(env.owner_id, TokenGrant::everything()),
                app.clock.time(),
            )
            .await
            .map_err(ApiError::internal)?;
            Ok(PortLocalData::List {
                ports: ports::list_view(&ctx, &mut conn, &principal, &env, query.clone()).await?,
            })
        }
        PortLocalRead::Address { port } => {
            if *port == 0 {
                return Err(ApiError::invalid("port", "a listener port is nonzero"));
            }
            let principal = Principal::load(
                &mut conn,
                orch_authz::Caller::token(env.owner_id, TokenGrant::everything()),
                app.clock.time(),
            )
            .await
            .map_err(ApiError::internal)?;
            Ok(PortLocalData::Address {
                target: ports::target(&ctx, &mut conn, &principal, &env, *port).await?,
            })
        }
        PortLocalRead::Peer {
            connection_id,
            port,
            transport,
            inside,
        } => {
            if *port == 0 || inside.port == 0 || inside.address.parse::<std::net::IpAddr>().is_err()
            {
                return Err(ApiError::invalid(
                    "peer",
                    "give a literal native peer and nonzero listener port",
                ));
            }
            drop(conn); // No database connection crosses the carrier/node round trip.
            let found = app
                .link
                .port_connections(
                    machine,
                    env.id,
                    ask.generation,
                    EnvironmentPortConnectionListQuery {
                        port: Some(*port),
                        inside_port: Some(inside.port),
                        transport: Some(*transport),
                        limit: Some(200),
                        ..Default::default()
                    },
                )
                .await
                .map_err(|_| {
                    ApiError::new(
                        ErrorCode::PortsUnavailable,
                        "the current carrier record is unavailable",
                    )
                })?;
            let row = found
                .connections
                .and_then(|rows| rows.into_iter().find(|row| row.id == *connection_id))
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::PortsUnavailable,
                        "the known inside peer has no current carrier confirmation",
                    )
                })?;
            let same_address = row
                .inside
                .address
                .parse::<std::net::IpAddr>()
                .ok()
                .map(|ip| ip.to_canonical())
                == inside
                    .address
                    .parse::<std::net::IpAddr>()
                    .ok()
                    .map(|ip| ip.to_canonical());
            let keys = port_entry::assertion_keys(app);
            if row.ended_at.is_some()
                || row.environment_id != env.id
                || row.generation != ask.generation
                || row.port != *port
                || row.transport != *transport
                || row.inside.port != inside.port
                || !same_address
            {
                return Ok(PortLocalData::Peer { peer: None, keys });
            }
            let mut tx = app.db.begin().await?;
            orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
            scope(&mut tx, machine, ask).await?;
            if matches!(row.outside.caller, PortCaller::Address) {
                let opening = match row.opening_id {
                    Some(id) => rows::opening(&mut tx, id).await?,
                    None => None,
                };
                if opening.is_none_or(|opening| {
                    opening.kind != "host"
                        || opening.environment_id != env.id
                        || opening.port != i32::from(*port)
                        || matches!(opening.state.as_str(), "waiting" | "closed" | "refused")
                        || opening.until_at.is_some_and(|at| at <= app.clock.now())
                }) {
                    return Ok(PortLocalData::Peer { peer: None, keys });
                }
            } else {
                tx.commit().await.map_err(orch_db::DbError::from)?;
                if !app.cache.ports().peer_current(&row).await? {
                    return Ok(PortLocalData::Peer { peer: None, keys });
                }
                tx = app.db.begin().await?;
                orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
                scope(&mut tx, machine, ask).await?;
            }
            let caller = match &row.outside.caller {
                PortCaller::Person { user } => AssertedCaller::Person {
                    id: user.id,
                    username: user.username.clone(),
                    display_name: user.display_name.clone(),
                },
                PortCaller::Link { id, label } => AssertedCaller::Link {
                    id: *id,
                    label: label.clone(),
                },
                PortCaller::Address => AssertedCaller::Address,
            };
            let assertion = port_entry::port_signer()
                .sign(
                    &PortAssertion {
                        issuer: app.config.system.public_url.clone(),
                        environment_id: env.id,
                        port: *port,
                        transport: match transport {
                            PortTransport::Tcp => "tcp",
                            PortTransport::Udp => "udp",
                        },
                        opening_id: row.opening_id,
                        connection_id: row.id,
                        generation: ask.generation,
                        outside_address: row.outside.address.clone(),
                        outside_port: row.outside.port,
                        caller,
                    },
                    app.clock.time(),
                )
                .map_err(|_| {
                    ApiError::new(
                        ErrorCode::PortsUnavailable,
                        "the current peer assertion is unavailable",
                    )
                })?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            Ok(PortLocalData::Peer {
                peer: Some(PortPeer {
                    connection: row,
                    assertion,
                }),
                keys,
            })
        }
        PortLocalRead::Keys => Ok(PortLocalData::Keys {
            keys: port_entry::assertion_keys(app),
        }),
    }
}

#[cfg(test)]
mod pending_tests {
    use super::*;
    use orch_types::new_id;

    #[test]
    fn pending_read_admission_is_per_environment_and_cancellation_releases_its_room() {
        let pending = Pending::default();
        let environment = new_id();
        let permits = (0..PENDING_PER_ENVIRONMENT)
            .map(|_| pending.enter(environment).unwrap())
            .collect::<Vec<_>>();
        assert!(pending.enter(environment).is_none());
        assert!(
            pending.enter(new_id()).is_some(),
            "one environment cannot fill another's sixteen slots"
        );
        drop(permits);
        assert!(pending.enter(environment).is_some());
        assert!(
            pending.0.lock().unwrap().is_empty(),
            "completed/cancelled calls leave no historical keys"
        );
    }
}
