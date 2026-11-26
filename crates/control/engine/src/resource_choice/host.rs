//! Native host feasibility supplied to the common pool walk and accepted choice journal.
mod refusals;
use super::{
    ChoiceRequest, Event, Hop, Kind, Leaf, Resolver, Target,
    database::{Database, Requirements},
};
use crate::{
    EngineError,
    operation_scope::Prepared,
    places::{self, Ask, HostPlace},
};
use async_trait::async_trait;
use futures::future::BoxFuture;
use orch_access_contract::pools::PathMember;
use orch_authz::Principal;
use orch_db::Conn;
use orch_types::{Id, Json};
use refusals::{Refusals, Why};

/// The actual native creation owns this identity; a preview has none and reserves nothing.
pub struct Request<'a> {
    pub ask: &'a Ask<'a>,
    pub target: &'a Target,
    pub config: &'a Json,
    pub bound: Option<&'a Prepared>,
    pub creation: Option<Id>,
}

pub struct Placement {
    pub place: HostPlace,
    pub choice: Option<Id>,
    pub path: Vec<Hop>,
}

/// A retained reference is restored only from this native creation's journal.
pub(crate) struct Retained {
    leaf: Leaf,
    path: Vec<PathMember>,
}
impl Retained {
    pub(crate) fn host(&self) -> Option<Id> {
        match &self.leaf {
            Leaf::Host { id } => Some(*id),
            Leaf::EnvironmentTemplate { host_id, .. } => Some(*host_id),
            _ => None,
        }
    }
    pub(crate) fn pools(&self) -> Vec<Id> {
        self.path.iter().map(|hop| hop.pool_id).collect()
    }
    fn selection(&self) -> orch_authz::pools::Selection<'_> {
        orch_authz::pools::Selection {
            leaf: &self.leaf,
            path: &self.path,
        }
    }
}

pub(crate) async fn retained(
    conn: &mut Conn,
    row: &orch_db::queries::jobs::JobRow,
    environment: &orch_db::queries::environments::hosting::EnvironmentState,
) -> Result<Option<Retained>, EngineError> {
    let Some(choice) = row.plan.get("pool_choice").filter(|value| !value.is_null()) else {
        if environment.generation == 0 {
            let required =
                row.plan.get("pool_choice_required").and_then(Json::as_bool) == Some(true);
            let accepted = orch_db::queries::pool_choices::get(conn, environment.id).await?;
            let pooled = accepted.is_some_and(|choice| {
                choice.source.get("kind") == Some(&serde_json::json!("environment_creation"))
                    && choice.source.get("environment") == Some(&serde_json::json!(environment.id))
            });
            let from_recipe: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM environment_pool_creations WHERE environment_id=$1)",
            )
            .bind(environment.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
            if required || pooled || from_recipe {
                return Err(EngineError::unreadable(
                    "the pending environment lost its admitted host pool reference",
                ));
            }
        }
        return Ok(None);
    };
    let choice: Id = serde_json::from_value(choice.clone())?;
    let stored = orch_db::queries::pool_choices::get(conn, choice)
        .await?
        .ok_or_else(|| EngineError::unreadable("the admitted host choice is gone"))?;
    let target: Target = serde_json::from_value(stored.target)?;
    let leaf: Leaf = serde_json::from_value(
        stored
            .leaf
            .ok_or_else(|| EngineError::unreadable("the host choice has no admitted member"))?,
    )?;
    let path: Vec<Hop> = serde_json::from_value(
        stored
            .path
            .ok_or_else(|| EngineError::unreadable("the host choice has no membership path"))?,
    )?;
    let source = stored.source;
    let configuration =
        serde_json::to_value(crate::jobs::command_identity::hash(&environment.config)?)?;
    let matches_creation = match (&target, &leaf) {
        (Target::Pool {kind:Kind::Host,..}, Leaf::Host {..}) => {
            (environment.generation != 0 || choice == environment.id)
                && source.get("kind") == Some(&serde_json::json!("environment_creation"))
                && source.get("environment") == Some(&serde_json::json!(environment.id))
                && source.get("node") == Some(&serde_json::json!(environment.home_node_id))
                && source.get("template") == Some(&serde_json::json!(environment.template_id))
                && source.get("configuration") == Some(&configuration)
        }
        (Target::Pool {kind:Kind::Environment,..}, Leaf::EnvironmentTemplate {template_id,host_id})
            if Some(*template_id) == environment.template_id
                && row.plan.get("admitted_host") == Some(&serde_json::json!(host_id))
                && source == serde_json::json!({"kind":"thread_creation","thread":choice,"node":environment.home_node_id,"purpose":"environment"}) => {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM environment_pool_creations WHERE choice_id=$1 AND environment_id=$2 AND job_id=$3 AND configuration=$4)")
                .bind(choice).bind(environment.id).bind(row.id).bind(&configuration)
                .fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?
        }
        _ => false,
    };
    if !matches_creation
        || stored.state != "selected"
        || row.environment_id != Some(environment.id)
        || stored.actor_id != environment.owner_id
        || !super::reserve::current_membership(conn, &target, &leaf, &path).await?
    {
        return Err(EngineError::unreadable(
            "the admitted host pool source changed",
        ));
    }
    Ok(Some(Retained {
        leaf,
        path: path
            .into_iter()
            .map(|hop| PathMember {
                pool_id: hop.pool_id,
                revision: hop.revision,
                member_id: hop.member_id,
            })
            .collect(),
    }))
}

/// Repeat complete current native feasibility at each pending provision fence, retaining its slot.
pub(crate) async fn current(
    conn: &mut Conn,
    principal: &Principal,
    row: &orch_db::queries::jobs::JobRow,
    environment: &orch_db::queries::environments::hosting::EnvironmentState,
    host: Id,
) -> Result<Option<HostPlace>, EngineError> {
    let Some(selected) = retained(conn, row, environment).await? else {
        return Ok(None);
    };
    if selected.host() != Some(host) {
        return Err(EngineError::unreadable(
            "the provision differs from its admitted host pool member",
        ));
    }
    let template = orch_db::queries::resources::hosting::template(
        conn,
        environment
            .template_id
            .ok_or_else(|| EngineError::unreadable("the new environment has no template"))?,
    )
    .await?
    .filter(|template| template.enabled && template.deleted_at.is_none())
    .ok_or_else(|| EngineError::unreadable("the admitted creation template is unavailable"))?;
    let wanted = places::wanted(
        conn,
        environment.home_node_id,
        template,
        &environment.config,
    )
    .await?;
    let minutes_idle = orch_settings::get(
        conn,
        orch_settings::Place::Environment(environment.id),
        orch_settings_contract::registry::keys::ENV_RECYCLE_IDLE_MINUTES,
    )
    .await?
    .unwrap_or(places::IDLE_MINUTES_DEFAULT);
    let ask = Ask {
        home: environment.home_node_id,
        wanted: &wanted,
        current: None,
        minutes_idle,
        now: principal.now().get(),
    };
    let mut listed = places::host_places_at_pool(
        conn,
        principal,
        &ask,
        selected.selection(),
        Some(environment.id),
    )
    .await?;
    let place = listed
        .pop()
        .filter(|place| {
            place.host.id == host
                && place.created.is_some()
                && !place.host.disabled
                && place.place.reasons.iter().all(|reason| {
                    matches!(
                        reason,
                        orch_api_contract::environment::HostReason::Full
                            | orch_api_contract::environment::HostReason::NoRoom
                    )
                })
        })
        .ok_or_else(|| {
            EngineError::unreadable("the selected host no longer permits this pooled creation")
        })?;
    crate::environments::validate_creation_host(
        conn,
        environment.home_node_id,
        &wanted,
        &environment.config,
        &place,
        false,
    )
    .await
    .map_err(|error| EngineError::unreadable(error.to_string()))?;
    Ok(Some(place))
}

struct Facts<'a, 'b> {
    request: &'a Request<'a>,
    refusals: &'b mut Refusals,
}
impl Facts<'_, '_> {
    async fn place(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        leaf: &Leaf,
        path: &[Hop],
    ) -> Result<Option<HostPlace>, EngineError> {
        let Leaf::Host { id } = leaf else {
            return Ok(None);
        };
        let path = path
            .iter()
            .map(|hop| PathMember {
                pool_id: hop.pool_id,
                revision: hop.revision,
                member_id: hop.member_id,
            })
            .collect::<Vec<_>>();
        let mut listed = if path.is_empty() {
            places::host_places_among(conn, principal, self.request.ask, &[*id]).await?
        } else {
            places::host_places_at_pool(
                conn,
                principal,
                self.request.ask,
                orch_authz::pools::Selection { leaf, path: &path },
                None,
            )
            .await?
        };
        if let Some(bound) = self.request.bound
            && matches!(self.request.target, Target::Pool { .. })
            && let Err(error) = crate::environment_creation::bound_candidates(
                conn,
                principal.caller(),
                bound,
                self.request.ask.wanted.template.id,
                &self.request.ask.wanted.mounts,
                &path.iter().map(|hop| hop.pool_id).collect::<Vec<_>>(),
                &mut listed,
            )
            .await
        {
            if matches!(error, crate::environments::CreateError::OperationScope) {
                return Ok(None);
            }
            return Err(EngineError::unreadable(error.to_string()));
        }
        Ok(listed.pop())
    }
}
#[async_trait]
impl Requirements for Facts<'_, '_> {
    async fn model(&mut self, _: &mut Conn, _: Id, _: &str) -> Result<Leaf, EngineError> {
        Err(EngineError::unreadable(
            "host placement cannot expand a model",
        ))
    }
    async fn reason(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
        leaf: &Leaf,
        path: &[Hop],
    ) -> Result<Option<String>, EngineError> {
        let Leaf::Host { id } = leaf else {
            return Ok(Some("candidate_purpose".into()));
        };
        let Some(place) = self.place(conn, principal, leaf, path).await? else {
            self.refusals
                .record(
                    conn,
                    principal,
                    self.request,
                    path,
                    *id,
                    Why::Other("your access does not allow creating this environment here"),
                )
                .await?;
            return Ok(Some("no_use".into()));
        };
        if !place.place.usable {
            if let Err(why) = places::choose(std::slice::from_ref(&place), Some(*id)) {
                self.refusals
                    .record(
                        conn,
                        principal,
                        self.request,
                        path,
                        *id,
                        Why::Scheduler(why),
                    )
                    .await?;
            }
            return Ok(Some("host_unavailable".into()));
        }
        match crate::environments::validate_creation_host(
            conn,
            self.request.ask.home,
            self.request.ask.wanted,
            self.request.config,
            &place,
            self.request.target.single_host().is_some(),
        )
        .await
        {
            Ok(()) => Ok(None),
            Err(crate::environments::CreateError::Internal(error)) => Err(error),
            Err(error) => {
                let (code, why) = match error {
                    crate::environments::CreateError::DataSources(_) => (
                        "data_sources",
                        "it cannot provide this environment’s data sources",
                    ),
                    crate::environments::CreateError::Runtime(_) => (
                        "runtime",
                        "its runtime does not meet this template’s requirements",
                    ),
                    _ => ("host_unavailable", "it cannot make this environment"),
                };
                self.refusals
                    .record(conn, principal, self.request, path, *id, Why::Other(why))
                    .await?;
                Ok(Some(code.into()))
            }
        }
    }
}

/// Preview walks current feasibility. Only an accepted creation draws or advances a policy.
pub fn select<'a>(
    conn: &'a mut Conn,
    principal: &'a Principal,
    request: &'a Request<'a>,
    refused: &'a mut Option<places::NoHost>,
) -> BoxFuture<'a, Result<Option<Placement>, EngineError>> {
    Box::pin(async move {
        let mut refusals = Refusals::default();
        let selected = select_member(conn, principal, request, &mut refusals).await?;
        if selected.is_none() {
            refusals.read_page(conn, principal).await?;
            *refused = refusals.into_result();
        }
        Ok(selected)
    })
}

async fn select_member(
    conn: &mut Conn,
    principal: &Principal,
    request: &Request<'_>,
    refusals: &mut Refusals,
) -> Result<Option<Placement>, EngineError> {
    request
        .target
        .validate(Kind::Host)
        .map_err(EngineError::unreadable)?;
    if let Target::Single { .. } = request.target {
        let mut walk = Resolver::new(
            Database::new(conn, principal, Facts { request, refusals }),
            request.target,
            Kind::Host,
        )
        .await?;
        while let Some(event) = walk.next().await? {
            if let Event::Candidate { leaf, path, .. } = event {
                drop(walk);
                return Ok(Facts { request, refusals }
                    .place(conn, principal, &leaf, &path)
                    .await?
                    .map(|place| Placement {
                        place,
                        choice: None,
                        path,
                    }));
            }
        }
        return Ok(None);
    }
    let (leaf, path, choice) = if let Some(creation) = request.creation {
        let source = serde_json::json!({"kind":"environment_creation","environment":creation,
            "node":request.ask.home,"purpose":"host","template":request.ask.wanted.template.id,
            "configuration":crate::jobs::command_identity::hash(request.config)?});
        let selected = Database::new(conn, principal, Facts { request, refusals })
            .reserve(&ChoiceRequest {
                id: creation,
                target: request.target.clone(),
                source,
            })
            .await?;
        let Some(leaf) = selected.leaf else {
            return Ok(None);
        };
        (leaf, selected.path, Some(selected.id))
    } else {
        let mut walk = Resolver::new(
            Database::new(conn, principal, Facts { request, refusals }),
            request.target,
            Kind::Host,
        )
        .await?;
        let mut feasible = None;
        while let Some(event) = walk.next().await? {
            if let Event::Candidate {
                leaf,
                path,
                reason: None,
            } = event
            {
                feasible = Some((leaf, path));
                break;
            }
        }
        let Some((leaf, path)) = feasible else {
            return Ok(None);
        };
        (leaf, path, None)
    };
    let facts = Facts { request, refusals };
    let place = facts.place(conn, principal, &leaf, &path).await?;
    Ok(place
        .filter(|place| place.place.usable)
        .map(|place| Placement {
            place,
            choice,
            path,
        }))
}
