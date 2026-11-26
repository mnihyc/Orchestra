//! Keep the first readable placement refusal from the native walk. A long run of hidden members
//! uses one page of scratch space and never exposes their names, states or count.
use super::{Hop, Request};
use crate::{EngineError, places::NoHost, resource_choice::visible};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::{environment::PlanHostBlocker, resources::HostRef};
use orch_authz::Principal;
use orch_db::{Conn, queries::resources::hosting};
use orch_provider_contract::pools::Target;
use orch_types::{Id, Time};

pub(super) enum Why {
    Scheduler(NoHost),
    Other(&'static str),
}

struct Candidate {
    host: Id,
    path: Vec<Id>,
    why: Why,
}

#[derive(Default)]
pub(super) struct Refusals {
    page_size: Option<usize>,
    page: Vec<Candidate>,
    found: Option<NoHost>,
}

impl Refusals {
    pub(super) async fn record(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
        request: &Request<'_>,
        path: &[Hop],
        host: Id,
        why: Why,
    ) -> Result<(), EngineError> {
        let Target::Pool { id, .. } = request.target else {
            return Ok(());
        };
        if self.found.is_some()
            || path.first().is_none_or(|hop| hop.pool_id != *id)
            || !visible::may_read(principal)
        {
            return Ok(());
        }
        let size = match self.page_size {
            Some(size) => size,
            None => {
                let size = orch_settings::get::<usize>(
                    conn,
                    orch_settings::Place::Pool(*id),
                    "pools.read_batch_size",
                )
                .await?
                .filter(|size| *size > 0)
                .ok_or_else(|| {
                    EngineError::unreadable("the pool needs a positive read batch size")
                })?;
                self.page_size = Some(size);
                size
            }
        };
        self.page.push(Candidate {
            host,
            path: path.iter().map(|hop| hop.pool_id).collect(),
            why,
        });
        if self.page.len() >= size {
            self.read_page(conn, principal).await?;
        }
        Ok(())
    }

    pub(super) async fn read_page(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
    ) -> Result<(), EngineError> {
        if self.page.is_empty() {
            return Ok(());
        }
        let names = visible::names(
            conn,
            principal,
            self.page
                .iter()
                .flat_map(|c| c.path.iter().copied().chain([c.host]))
                .collect(),
        )
        .await?;
        for candidate in self.page.drain(..) {
            let Some(name) = names.get(&(ObjectKind::Host, candidate.host)) else {
                continue;
            };
            if !candidate
                .path
                .iter()
                .all(|pool| names.contains_key(&(ObjectKind::Pool, *pool)))
            {
                continue;
            }
            self.found = Some(match candidate.why {
                Why::Scheduler(refusal) => refusal,
                Why::Other(reason) => {
                    let Some(host) = hosting::host(conn, candidate.host).await? else {
                        continue;
                    };
                    let message = format!("{name}: {reason}");
                    NoHost {
                        code: "no_host",
                        message: message.clone(),
                        blockers: vec![PlanHostBlocker {
                            host: HostRef {
                                id: host.id,
                                name: name.clone(),
                                address: String::new(),
                            },
                            online: host.online,
                            offline_since: if host.online {
                                None
                            } else {
                                host.offline_since.map(Time::new)
                            },
                            reasons: vec![],
                            message,
                        }],
                    }
                }
            });
            break;
        }
        Ok(())
    }

    pub(super) fn into_result(self) -> Option<NoHost> {
        self.found
    }
}
