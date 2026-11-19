//! Live whole-row pages from the gateway and host nodes, combined without a connection-history table.

use super::{EntryError, Link};
use orch_api_contract::ports::{
    EnvironmentPortConnectionListQuery, PortCallerTotal, PortConnectionList, PortConnectionOwner,
};
use orch_types::Id;

// A rapid sequence of moves must not turn one live read into unbounded machine queries.
const RECORD_OWNERS_MAX: u32 = 16;
// The public live-record contract admits whole-row pages of at most two hundred rows.
const PAGE_MAX: u32 = 200;
const PAGE_DEFAULT: u32 = 50;

impl Link {
    /// Reads one already-authorized host node over its current session; used by the combined projection.
    pub async fn node_port_connections(
        &self,
        node: Id,
        environment: Id,
        generation: i64,
        query: EnvironmentPortConnectionListQuery,
    ) -> Result<PortConnectionList, EntryError> {
        self.carrier_port_connections(node, environment, generation, query, true)
            .await
    }
    /// Reads each actual carrier's bounded current memory. An unavailable owner is not a false zero.
    pub async fn port_connections(
        &self,
        machine: Id,
        environment: Id,
        generation: i64,
        query: EnvironmentPortConnectionListQuery,
    ) -> Result<PortConnectionList, EntryError> {
        let nodes = {
            let mut conn = self.inner.db.acquire().await?;
            orch_db::queries::ports::record_nodes(
                &mut conn,
                environment,
                self.inner.clock.now()
                    - chrono::Duration::seconds(i64::from(
                        self.inner.settings.limits.ports.recent_secs,
                    )),
                RECORD_OWNERS_MAX + 1,
            )
            .await?
        };
        // Sixteen distinct target numbers are shipped; a rapid sequence of host changes can
        // leave more historical owners. Refuse an excessive fanout rather than inventing totals.
        if nodes.len() > RECORD_OWNERS_MAX as usize {
            return Err(EntryError::Database);
        }
        if nodes.is_empty() {
            return self
                .carrier_port_connections(machine, environment, generation, query, false)
                .await;
        }
        let limit = query.limit.unwrap_or(PAGE_DEFAULT).clamp(1, PAGE_MAX);
        let mut page = query;
        page.limit = Some(limit.saturating_add(1).min(PAGE_MAX));
        let gateway = self
            .carrier_port_connections(machine, environment, generation, page.clone(), false)
            .await?;
        let mut parts = vec![gateway];
        parts.extend(
            futures::future::try_join_all(nodes.into_iter().map(|node| {
                self.node_port_connections(node, environment, generation, page.clone())
            }))
            .await?,
        );
        let mut result = merge(parts, limit as usize)?;
        // Historical nodes may overlap after a move. The returned caller inventory still has
        // the environment's one configured total; overflow remains numeric Other, never identities.
        if let Some(totals) = &mut result.totals {
            let bound = orch_link::ports::count(self.inner.settings.limits.ports.caller_totals);
            let (mut known, others): (Vec<PortCallerTotal>, Vec<PortCallerTotal>) =
                totals.drain(..).partition(|row| !row.other);
            known.sort_by_key(|row| std::cmp::Reverse(row.last_active));
            let mut overflow = if known.len() > bound {
                known.split_off(bound)
            } else {
                Vec::new()
            };
            if !overflow.is_empty() {
                result.truncated = true;
            }
            overflow.extend(others);
            let mut other = None::<PortCallerTotal>;
            for mut row in overflow {
                row.caller = None;
                row.address = None;
                row.other = true;
                if let Some(known) = &mut other {
                    combine_total(known, &row);
                } else {
                    other = Some(row);
                }
            }
            if let Some(other) = other {
                known.push(other);
            }
            *totals = known;
        }
        Ok(result)
    }
}

fn optional_sum(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (None, None) => None,
        (known, other) => known.or(other),
    }
}

fn merge(
    mut parts: Vec<PortConnectionList>,
    limit: usize,
) -> Result<PortConnectionList, EntryError> {
    let Some(mut result) = parts.pop() else {
        return Err(EntryError::Database);
    };
    let mut rows = result.connections.take().ok_or(EntryError::Database)?;
    let mut totals = result.totals.take().ok_or(EntryError::Database)?;
    let mut more = result.cursor.is_some();
    for part in parts {
        more |= part.cursor.is_some();
        result.truncated |= part.truncated;
        result.as_of = result.as_of.min(part.as_of);
        result.counts.active = result.counts.active.saturating_add(part.counts.active);
        result.counts.recent = result.counts.recent.saturating_add(part.counts.recent);
        result.counts.bytes_in = result.counts.bytes_in.saturating_add(part.counts.bytes_in);
        result.counts.bytes_out = result
            .counts
            .bytes_out
            .saturating_add(part.counts.bytes_out);
        result.counts.http_requests =
            optional_sum(result.counts.http_requests, part.counts.http_requests);
        // Mixed owners began independently; show the earliest known beginning and latest reset.
        result.counts.tracked_since = result.counts.tracked_since.min(part.counts.tracked_since);
        result.counts.reset_at = result.counts.reset_at.max(part.counts.reset_at);
        rows.extend(part.connections.ok_or(EntryError::Database)?);
        for total in part.totals.ok_or(EntryError::Database)? {
            add_total(&mut totals, total);
        }
    }
    rows.sort_by_key(|row| row.id);
    rows.dedup_by_key(|row| row.id);
    more |= rows.len() > limit;
    rows.truncate(limit);
    result.cursor = if more {
        rows.last().map(|row| row.id.to_string())
    } else {
        None
    };
    result.owner = PortConnectionOwner::Mixed;
    result.connections = Some(rows);
    result.totals = Some(totals);
    result.truncated |= more;
    Ok(result)
}

fn add_total(totals: &mut Vec<PortCallerTotal>, total: PortCallerTotal) {
    if let Some(known) = totals.iter_mut().find(|known| {
        known.caller == total.caller && known.address == total.address && known.other == total.other
    }) {
        combine_total(known, &total);
    } else {
        totals.push(total);
    }
}

fn combine_total(known: &mut PortCallerTotal, total: &PortCallerTotal) {
    known.active = known.active.saturating_add(total.active);
    known.connections = known.connections.saturating_add(total.connections);
    known.bytes_in = known.bytes_in.saturating_add(total.bytes_in);
    known.bytes_out = known.bytes_out.saturating_add(total.bytes_out);
    known.http_requests = optional_sum(known.http_requests, total.http_requests);
    known.last_active = known.last_active.max(total.last_active);
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::ports::{
        PortCaller, PortConnection, PortConnectionInside, PortConnectionOutside, PortCounts,
        PortTransport,
    };
    use orch_types::Time;

    fn page(owner: PortConnectionOwner, ids: &[u128], at: &str) -> PortConnectionList {
        let at: Time = at.parse().unwrap();
        let rows = ids
            .iter()
            .map(|id| PortConnection {
                id: Id::from_u128(*id),
                opening_id: None,
                environment_id: Id::from_u128(1),
                port: 3000,
                transport: PortTransport::Tcp,
                generation: 1,
                outside: PortConnectionOutside {
                    address: "192.0.2.7".into(),
                    port: 41000,
                    caller: PortCaller::Address,
                },
                inside: PortConnectionInside {
                    address: "127.0.0.1".into(),
                    port: 41001,
                },
                since: at,
                last_active: at,
                bytes_in: 7,
                bytes_out: 5,
                http_requests: None,
                ended_at: None,
            })
            .collect();
        let count = ids.len() as i64;
        PortConnectionList {
            as_of: at,
            owner,
            counts: PortCounts {
                active: count,
                recent: 0,
                http_requests: None,
                bytes_in: count * 7,
                bytes_out: count * 5,
                tracked_since: at,
                reset_at: at,
            },
            connections: Some(rows),
            totals: Some(vec![PortCallerTotal {
                caller: Some(PortCaller::Address),
                address: Some("192.0.2.7".into()),
                active: count,
                connections: count,
                bytes_in: count * 7,
                bytes_out: count * 5,
                http_requests: None,
                last_active: at,
                other: false,
            }]),
            cursor: None,
            truncated: false,
        }
    }

    #[test]
    fn a_manager_gets_one_ordered_whole_row_page_with_both_owners_counts_and_the_earliest_measurement()
     {
        let first = page(
            PortConnectionOwner::Gateway,
            &[2, 4],
            "2026-10-14T20:00:00.000Z",
        );
        let last = page(
            PortConnectionOwner::Node,
            &[3, 5],
            "2026-10-14T20:00:01.000Z",
        );
        let combined = merge(vec![first.clone(), last.clone()], 2).unwrap();
        assert_eq!(combined.owner, PortConnectionOwner::Mixed);
        assert_eq!(combined.as_of, first.as_of);
        assert_eq!(combined.counts.active, 4);
        assert_eq!(combined.counts.bytes_in, 28);
        assert_eq!(
            combined.counts.http_requests, None,
            "raw-host traffic is not an HTTP request estimate"
        );
        assert_eq!(combined.counts.tracked_since, first.as_of);
        assert_eq!(combined.counts.reset_at, last.as_of);
        assert_eq!(
            combined
                .connections
                .as_ref()
                .unwrap()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            vec![Id::from_u128(2), Id::from_u128(3)]
        );
        assert_eq!(combined.cursor, Some(Id::from_u128(3).to_string()));
        assert!(combined.truncated);
        let totals = combined.totals.unwrap();
        assert_eq!(totals.len(), 1);
        assert_eq!(totals[0].active, 4);
        // A continuation is applied by each real owner before combining, so no first-page row repeats.
        let next = merge(
            vec![
                page(
                    PortConnectionOwner::Gateway,
                    &[4],
                    "2026-10-14T20:00:00.000Z",
                ),
                page(PortConnectionOwner::Node, &[5], "2026-10-14T20:00:01.000Z"),
            ],
            2,
        )
        .unwrap();
        assert_eq!(
            next.connections
                .unwrap()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            vec![Id::from_u128(4), Id::from_u128(5)]
        );
        assert!(next.cursor.is_none());
    }

    #[test]
    fn a_missing_carrier_inventory_is_unavailable_instead_of_fabricated_zero() {
        let mut unavailable = page(PortConnectionOwner::Node, &[], "2026-10-14T20:00:00.000Z");
        unavailable.connections = None;
        assert!(merge(vec![unavailable], 50).is_err());
        assert!(merge(Vec::new(), 50).is_err());
    }
}
