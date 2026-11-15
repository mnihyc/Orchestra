//! Physical work facts retained by the node; only a current controller command admits their use.
use orch_driver_contract::units::UnitRef;
use orch_link_contract::program_work::Admission;
use orch_types::{Hash, Id};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The node's actual prepared unit and native process references, without any input or sign-in bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    /// The immutable controller admission facts.
    pub admission: Admission,
    /// Bound after the driver prepared frozen material.
    pub unit: Option<UnitRef>,
    /// Exact material identity, never the material itself.
    pub input: Option<Hash>,
    /// Each logical invocation retains its actual prepared driver identity before start.
    pub processes: BTreeMap<Id, String>,
    /// Retirement is terminal for this work identity.
    pub retired: bool,
}

impl Record {
    /// Conjunctive physical matching after the link verified the ticket and its presenting machine.
    /// Claims parsed from JSON alone are never an admission to this service.
    #[must_use]
    pub fn accepts_channel(
        &self,
        claims: &orch_link_contract::tunnels::TicketClaims,
        presenter: orch_link_contract::tunnels::TicketSource,
        now: orch_types::Time,
    ) -> bool {
        use orch_link_contract::tunnels::{Service, TicketSource, TunnelKind};
        !self.retired
            && now < self.admission.deadline
            && presenter == TicketSource::Machine(self.admission.peer)
            && claims.kind == TunnelKind::Program
            && claims.thread_id == Some(self.admission.helper)
            && claims.service.parse::<Service>().is_ok_and(|service| {
                service
                    == Service::Program {
                        work_id: self.admission.work,
                    }
            })
            && claims.exp <= self.admission.deadline.get().timestamp()
            && claims.check(presenter, now).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_link_contract::tunnels::{Service, TicketClaims, TicketSource, TunnelKind};
    use orch_types::{Time, new_id};
    #[test]
    fn a_program_channel_reaches_only_its_admitted_helper_peer_and_live_work() {
        let now: Time = "2026-10-11T12:00:00Z".parse().unwrap();
        let deadline: Time = "2026-10-11T12:01:00Z".parse().unwrap();
        let record = Record {
            admission: Admission {
                work: new_id(),
                helper: new_id(),
                peer: new_id(),
                parent: new_id(),
                template: serde_json::json!({}),
                tools: vec![],
                endpoints: vec![],
                deadline,
            },
            unit: None,
            input: None,
            processes: BTreeMap::new(),
            retired: false,
        };
        let claims = TicketClaims {
            jti: new_id(),
            kind: TunnelKind::Program,
            src: TicketSource::Machine(record.admission.peer),
            dst: new_id(),
            service: Service::Program {
                work_id: record.admission.work,
            }
            .to_string(),
            thread_id: Some(record.admission.helper),
            tool_set: None,
            tool_environment_id: None,
            tool_environment_generation: None,
            tool_executor_version: None,
            port: None,
            exp: deadline.get().timestamp(),
        };
        assert!(record.accepts_channel(&claims, claims.src, now));
        assert!(!record.accepts_channel(&claims, TicketSource::Machine(new_id()), now));
        let mut other = claims.clone();
        other.thread_id = Some(new_id());
        assert!(!record.accepts_channel(&other, other.src, now));
        other = claims.clone();
        other.service = Service::Program { work_id: new_id() }.to_string();
        assert!(!record.accepts_channel(&other, other.src, now));
        other = claims.clone();
        other.exp += 1;
        assert!(!record.accepts_channel(&other, other.src, now));
        assert!(!record.accepts_channel(&claims, claims.src, deadline));
        let mut retired = record;
        retired.retired = true;
        assert!(!retired.accepts_channel(&claims, claims.src, now));
    }
}
