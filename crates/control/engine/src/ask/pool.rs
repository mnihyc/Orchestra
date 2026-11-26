//! The accepted answer owns its draw across native steps and controller recovery.
use super::{model, policy};
use crate::{EngineError, resource_choice::ProviderSelection};
use orch_access_contract::permissions::UsagePurpose;
use orch_authz::Principal;
use orch_db::{Conn, DbError};
use orch_provider_contract::capabilities::BackendRef;
use orch_types::Id;

use orch_provider_contract::pools::{Kind, Leaf, Target};

fn source(root: Id, thread: Id, turn: Id, model: Option<&str>) -> orch_types::Json {
    let mut source =
        serde_json::json!({"kind":"ask","node":thread,"root":root,"turn":turn,"purpose":"ask"});
    if let Some(model) = model {
        source["model"] = serde_json::json!(model);
    }
    source
}

pub(super) async fn retained(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    turn: Id,
    backend: &BackendRef,
) -> Result<Option<ProviderSelection>, model::Error> {
    let row: Option<(Id, Id, Id, orch_types::Json)> = sqlx::query_as(
        "SELECT chosen.id, turns.thread_id, chosen.actor_id, chosen.source
         FROM ask_turn_pool_bindings binding JOIN turns ON turns.id=binding.turn_id
         JOIN pool_choices chosen ON chosen.id=binding.choice_id WHERE binding.turn_id=$1",
    )
    .bind(turn)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    let Some((id, thread, actor, origin)) = row else {
        return Ok(None);
    };
    let refused = || {
        model::Error::Denied(model::Refusal {
            reason: "selected_backend_unavailable".into(),
            message: "The pool member selected for this answer is no longer available.".into(),
        })
    };
    if actor != principal.caller().user() {
        return Err(refused());
    }
    let offering = backend.offering.as_ref().ok_or_else(refused)?;
    let selected = ProviderSelection::load(
        conn,
        id,
        thread,
        offering.provider,
        Some(&offering.offering),
        UsagePurpose::Ask,
        principal.caller(),
    )
    .await?;
    let leaf = selected.selection().leaf;
    let requested = matches!(leaf, Leaf::Provider { .. }).then_some(offering.offering.as_str());
    if origin != source(root, thread, turn, requested) || !policy::includes(leaf, backend) {
        return Err(refused());
    }
    Ok(Some(selected))
}

/// Link the admitted turn to its choice in the transaction that creates its input.
pub async fn bind(conn: &mut Conn, turn: Id, choice: Id) -> Result<(), EngineError> {
    sqlx::query("INSERT INTO ask_turn_pool_bindings(turn_id, choice_id) VALUES ($1,$2)")
        .bind(turn)
        .bind(choice)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

fn ask_backend(leaf: &Leaf, requested: Option<&str>) -> Option<BackendRef> {
    use orch_api_contract::ask::{API_ROUTE, AskModelIdentity, PROGRAM_ROUTE};
    if let Leaf::Provider { id } = leaf {
        let model = requested.filter(|model| !model.trim().is_empty())?;
        return Some(AskModelIdentity::api(*id, model.to_owned()).backend);
    }
    let Leaf::Model { backend } = leaf else {
        return None;
    };
    match backend.route.as_str() {
        API_ROUTE | PROGRAM_ROUTE => Some(backend.clone()),
        "generation.model" => {
            let offering = backend.offering.as_ref()?;
            let exact = AskModelIdentity::api(offering.provider, offering.offering.clone()).backend;
            policy::includes(leaf, &exact).then_some(exact)
        }
        _ => None,
    }
}

struct Candidates<'a> {
    policy: policy::Policy,
    root: Id,
    pool: Id,
    requested: Option<&'a str>,
    params: Option<&'a orch_types::Json>,
}

impl Candidates<'_> {
    async fn check(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        leaf: &orch_provider_contract::pools::Leaf,
        path: &[crate::resource_choice::Hop],
    ) -> Result<model::Selection, model::Error> {
        let backend = ask_backend(leaf, self.requested).ok_or_else(|| {
            model::Error::denied(
                "generation_unsupported",
                "This pool member offers no installed Ask interface.",
            )
        })?;
        if !self
            .policy
            .permits(conn, principal, &backend, Some((leaf.kind(), self.pool)))
            .await?
        {
            return Err(model::Error::denied(
                "excluded_by_policy",
                "An Ask policy excludes this pool member.",
            ));
        }
        let selected =
            ProviderSelection::prospective(leaf, path, UsagePurpose::Ask, principal.caller())
                .ok_or_else(|| {
                    EngineError::unreadable("An Ask pool member has no selection path")
                })?;
        model::select_offering(
            conn,
            principal,
            self.root,
            orch_api_contract::ask::AskModel {
                backend,
                params: self.params.cloned(),
            },
            principal.now(),
            Some(&selected),
        )
        .await
    }
}

#[async_trait::async_trait]
impl crate::resource_choice::database::Requirements for &Candidates<'_> {
    async fn model(
        &mut self,
        _: &mut Conn,
        provider: Id,
        model: &str,
    ) -> Result<orch_provider_contract::pools::Leaf, EngineError> {
        Ok(orch_provider_contract::pools::Leaf::generation_model(
            provider, model,
        ))
    }
    async fn reason(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
        leaf: &orch_provider_contract::pools::Leaf,
        path: &[crate::resource_choice::Hop],
    ) -> Result<Option<String>, EngineError> {
        let check = async {
            let selected = self.check(conn, principal, leaf, path).await?;
            let tier = selected
                .protocol
                .as_deref()
                .zip(selected.model.params.as_ref())
                .and_then(|(protocol, params)| {
                    orch_model_contract::parameters::requested_tier_for_caps(protocol, params)
                });
            let work = crate::caps::OwnCapAsk {
                place: self.root,
                provider: selected.offering.provider,
                model: &selected.offering.offering,
                tier: tier.as_deref(),
                purpose: "ask",
                person: Some(principal.caller().user()),
            };
            if !crate::caps::ask_candidate_room(conn, &work, principal.now().get()).await? {
                return Err(model::Error::denied(
                    "cap_reached",
                    "A usage limit for Ask has been reached.",
                ));
            }
            selected.online(conn).await
        }
        .await;
        match check {
            Ok(_) => Ok(None),
            Err(model::Error::Denied(reason)) => Ok(Some(reason.reason)),
            Err(model::Error::Db(error)) => Err(error.into()),
            Err(model::Error::Native(error)) => Err(error),
        }
    }
}

/// Reserve this authored answer's member, under the same transaction as its input and turn.
/// A returned refusal must abort the transaction, including all rotation and choice records.
pub async fn reserve(
    engine: &crate::Engine,
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    thread: Id,
    turn: Id,
    kind: Kind,
    pool: Id,
    requested: Option<&str>,
    params: Option<&orch_types::Json>,
) -> Result<model::Chosen, model::Error> {
    use crate::resource_choice::{ChoiceRequest, database::Database};
    let facts = Candidates {
        policy: policy::Policy::load(conn, root).await?,
        root,
        pool,
        requested,
        params,
    };
    let choice = Database::new(conn, principal, &facts)
        .reserve(&ChoiceRequest {
            id: turn,
            target: Target::Pool { id: pool, kind },
            source: source(root, thread, turn, requested),
        })
        .await?;
    let leaf = choice.leaf.as_ref().ok_or_else(|| {
        model::Error::denied(
            "selected_backend_unavailable",
            "No eligible member of this Ask pool can answer the question.",
        )
    })?;
    facts
        .check(conn, principal, leaf, &choice.path)
        .await?
        .available(engine, conn, root, principal.caller().user(), None)
        .await
}

/// The order set by the controlling scope, narrowed by each contributing Ask policy.
pub async fn entries(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
) -> Result<Vec<orch_api_contract::ask::AskTarget>, EngineError> {
    policy::Policy::load(conn, root)
        .await?
        .entries(conn, principal)
        .await
}

/// Only disclose the name to a reader who may view this pool. Preview its current
/// availability without selecting a member, writing cap notices or moving rotation.
pub async fn preview(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    kind: Kind,
    id: Id,
    requested: Option<&str>,
) -> Result<Option<(String, bool)>, EngineError> {
    use crate::resource_choice::{Event, Resolver, database::Database};
    use orch_access_contract::{
        decisions::ObjectRef,
        permissions::{ObjectKind, Permission},
    };
    if !orch_authz::resources::check(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Pool,
            id,
        },
        Permission::PoolView,
        Default::default(),
    )
    .await?
    .is_allowed()
    {
        return Ok(None);
    }
    let Some(row) = orch_db::queries::pools::get(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none() && row.kind == kind.as_str())
    else {
        return Ok(None);
    };
    let facts = Candidates {
        policy: policy::Policy::load(conn, root).await?,
        root,
        pool: id,
        requested,
        params: None,
    };
    let mut resolver = Resolver::new(
        Database::new(conn, principal, &facts),
        &Target::Pool { kind, id },
        kind,
    )
    .await?;
    while let Some(event) = resolver.next().await? {
        if matches!(event, Event::Candidate { reason: None, .. }) {
            return Ok(Some((row.name, true)));
        }
    }
    Ok(Some((row.name, false)))
}
