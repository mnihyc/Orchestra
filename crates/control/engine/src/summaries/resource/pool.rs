//! A resource refresh owns its pool choice from acceptance through publication.
use super::*;
use crate::resource_choice::{
    Event, Hop, ProviderSelection, Resolver,
    database::{Database, Requirements},
};
use orch_access_contract::permissions::UsagePurpose;
use orch_provider_contract::pools::{Kind, Leaf, Target};
use orch_summarizer_contract::descriptor::SummaryField;
use serde_json::json;

pub fn configured(policy: &options::Policy) -> Result<Option<Id>, EngineError> {
    match policy.producer.as_ref().filter(|p| p.kind == "model") {
        Some(producer) => Config::pool_id(&producer.config).map_err(EngineError::unreadable),
        None => Ok(None),
    }
}

struct Preview<'a> {
    node: Id,
    policy: &'a options::Policy,
    field: SummaryField,
}

impl Preview<'_> {
    fn config(&self, leaf: &Leaf) -> Result<Option<Config>, EngineError> {
        let Some(backend) = super::super::pool::backend(leaf) else {
            return Ok(None);
        };
        let producer = self.policy.producer.as_ref().ok_or_else(refused)?;
        let mut config = Config::with_pool_options(&producer.config, backend)
            .map_err(EngineError::unreadable)?;
        config.context_tokens = self.policy.context_tokens.get();
        Ok(Some(config))
    }
}

#[async_trait::async_trait]
impl Requirements for Preview<'_> {
    async fn model(
        &mut self,
        _: &mut Conn,
        provider: Id,
        model: &str,
    ) -> Result<Leaf, EngineError> {
        Ok(Leaf::generation_model(provider, model))
    }
    async fn reason(
        &mut self,
        conn: &mut Conn,
        principal: &Principal,
        leaf: &Leaf,
        path: &[Hop],
    ) -> Result<Option<String>, EngineError> {
        let Some(config) = self.config(leaf)? else {
            return Ok(Some("candidate_purpose".into()));
        };
        let Some(selection) =
            ProviderSelection::prospective(leaf, path, UsagePurpose::Summary, principal.caller())
        else {
            return Ok(Some("candidate_not_allowed".into()));
        };
        if judge_model_at(conn, principal, Some(self.node), &config, Some(&selection))
            .await?
            .is_err()
        {
            return Ok(Some("candidate_not_allowed".into()));
        }
        match super::super::budget::context_tokens(conn, &config, self.field).await {
            Ok(_) => Ok(None),
            Err(EngineError::Unreadable(_)) => Ok(Some("candidate_parameters".into())),
            Err(error) => Err(error),
        }
    }
}

/// Find an available native route for effect-free planning. No pool cursor or choice is written.
pub async fn preview(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    policy: &options::Policy,
    target: SummaryTarget,
    id: Id,
) -> Result<Option<(Config, ProviderSelection)>, EngineError> {
    let members = || Preview {
        node: place,
        policy,
        field: field_for(target),
    };
    let mut resolver = Resolver::new(
        Database::new(conn, principal, members()),
        &Target::Pool {
            id,
            kind: Kind::Model,
        },
        Kind::Model,
    )
    .await?;
    while let Some(event) = resolver.next().await? {
        if let Event::Candidate {
            leaf,
            path,
            reason: None,
        } = event
        {
            let member = ProviderSelection::prospective(
                &leaf,
                &path,
                UsagePurpose::Summary,
                principal.caller(),
            );
            return Ok(members().config(&leaf)?.zip(member));
        }
    }
    Ok(None)
}

fn source(
    request: Id,
    issuer: Option<Id>,
    payer: Id,
    node: Id,
    target: SummaryTarget,
    policy: Hash,
) -> Json {
    json!({"kind":"resource_summary","purpose":"summary","node":node,"request":request,"target":target,"policy":policy,"issuer":issuer,"payer":payer})
}

/// Only newly accepted rows reach this method on the accepting transaction.
pub async fn accept(
    engine: &Engine,
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::Request,
    selected: &Selection,
) -> Result<(), EngineError> {
    let Some(id) = configured(&selected.policy)? else {
        return Ok(());
    };
    let payer = row
        .paying_user
        .filter(|user| *user == principal.caller().user())
        .ok_or_else(refused)?;
    let producer = selected.policy.producer.as_ref().ok_or_else(refused)?;
    let origin = source(
        row.id,
        row.requested_by,
        payer,
        selected.material.identity.place(),
        selected.material.identity.target(),
        selected.policy.fingerprint,
    );
    let config = super::super::pool::reserve_resource(
        engine,
        conn,
        principal,
        row,
        selected,
        id,
        &producer.config,
        origin,
    )
    .await?;
    let mut accepted = row.accepted_source.clone();
    accepted["selection"] = serde_json::to_value(ModelSelection::configured(&config))?;
    accepted["model_pool"] = json!(id);
    let count = sqlx::query("UPDATE resource_summary_requests SET accepted_source=$2 WHERE id=$1 AND accepted_source=$3")
        .bind(row.id).bind(accepted).bind(&row.accepted_source).execute(conn).await.map_err(DbError::from)?.rows_affected();
    if count != 1 {
        return Err(refused());
    }
    Ok(())
}

pub(super) async fn retained(
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::Request,
    identity: &sources::Identity,
    policy: &options::Policy,
) -> Result<Option<ProviderSelection>, EngineError> {
    let Some(pool) = row.accepted_source.get("model_pool") else {
        return Ok(None);
    };
    let pool: Id = serde_json::from_value(pool.clone())?;
    let payer = row
        .paying_user
        .filter(|user| *user == principal.caller().user())
        .ok_or_else(refused)?;
    if configured(policy)? != Some(pool) {
        return Err(refused());
    }
    let node = identity.place();
    let expected = source(
        row.id,
        row.requested_by,
        payer,
        node,
        identity.target(),
        policy.fingerprint,
    );
    let matches: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pool_choices WHERE id=$1 AND actor_id=$2 AND source=$3 AND state='selected')")
        .bind(row.id).bind(payer).bind(expected).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    if !matches {
        return Err(refused());
    }
    let original: ModelSelection =
        serde_json::from_value(row.accepted_source["selection"].clone())?;
    let offering = original.offering()?;
    let selection = ProviderSelection::load(
        conn,
        row.id,
        node,
        offering.provider,
        Some(&offering.offering),
        UsagePurpose::Summary,
        principal.caller(),
    )
    .await?;
    if !selection.targets(&Target::Pool {
        id: pool,
        kind: Kind::Model,
    }) || !super::super::pool::allowed(
        conn,
        principal,
        node,
        &selection,
        &original.backend,
        Some(&original.params),
    )
    .await?
    {
        return Err(refused());
    }
    Ok(Some(selection))
}
