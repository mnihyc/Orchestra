//! Provider birth supplies native feasibility to the common choice journal, never a retry loop.
use super::*;
use crate::resource_choice::{
    ChoiceRequest, Hop,
    database::{Database, Requirements},
};
use futures::future::BoxFuture;
use orch_access_contract::pools::PathMember;
use serde::{Deserialize, Serialize};
mod command;
pub use command::Command;
pub(crate) use command::decorate;
use orch_provider_contract::pools::{Kind, Leaf, Target};

struct Facts<'a> {
    provider: &'a State,
    template: &'a TemplateState,
    pinned: Option<Id>,
}
fn path(hops: &[Hop]) -> Vec<PathMember> {
    hops.iter()
        .map(|hop| PathMember {
            pool_id: hop.pool_id,
            revision: hop.revision,
            member_id: hop.member_id,
        })
        .collect()
}
#[async_trait]
impl Requirements for Facts<'_> {
    async fn pool_reason(
        &mut self,
        conn: &mut orch_db::Conn,
        principal: &Principal,
        id: Id,
    ) -> Result<Option<String>, EngineError> {
        let policies = crate::access::resource_policies_among(conn, &[id]).await?;
        let allowed = if let Some(birth) =
            orch_authz::resources::ProviderBirth::current(conn, principal, self.provider.id).await?
        {
            !orch_authz::pools::pool_allowances_for_birth(
                conn,
                principal,
                id,
                Some(self.provider.home_id),
                &policies,
                &birth,
            )
            .await?
            .is_empty()
        } else {
            matches!(
                orch_authz::usable::usable(
                    conn,
                    principal,
                    id,
                    Some(self.provider.home_id),
                    &policies
                )
                .await?,
                orch_authz::usable::Usable::Yes(_)
            )
        };
        Ok((!allowed).then(|| "no_use".into()))
    }
    async fn model(&mut self, _: &mut orch_db::Conn, _: Id, _: &str) -> Result<Leaf, EngineError> {
        Err(EngineError::unreadable(
            "provider host placement cannot expand a model",
        ))
    }
    async fn reason(
        &mut self,
        conn: &mut orch_db::Conn,
        principal: &Principal,
        leaf: &Leaf,
        hops: &[Hop],
    ) -> Result<Option<String>, EngineError> {
        let Leaf::Host { id } = leaf else {
            return Ok(Some("member_kind".into()));
        };
        if self.pinned.is_some_and(|pinned| pinned != *id) {
            return Ok(Some("existing_storage".into()));
        }
        let path = path(hops);
        let selection = orch_authz::pools::Selection { leaf, path: &path };
        let candidates = candidates_scoped(
            conn,
            self.provider,
            self.template,
            Some(*id),
            principal,
            Some(selection),
        )
        .await?;
        if candidates
            .iter()
            .all(|candidate| candidate.host.id != *id || !candidate.usable())
        {
            return Ok(Some("host_unavailable".into()));
        }
        Ok(placement_bounds(
            conn,
            principal,
            self.provider,
            self.template,
            *id,
            Some(selection),
        )
        .await?
        .is_none()
        .then(|| "no_use".into()))
    }
}

pub(super) struct Selected {
    pub candidate: Candidate,
    pub created: orch_authz::environments::Created,
}

/// Effect-free feasibility uses the same request adapter without reserving policy or a native unit.
pub async fn preview(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    provider: &State,
    template: &TemplateState,
    target: &Target,
    excluded: Option<Id>,
) -> Result<bool, EngineError> {
    let mut walk = crate::resource_choice::Resolver::new(
        Database::new(
            conn,
            principal,
            Facts {
                provider,
                template,
                pinned: None,
            },
        ),
        target,
        Kind::Host,
    )
    .await?;
    while let Some(event) = walk.next().await? {
        if let crate::resource_choice::Event::Candidate {
            leaf: Leaf::Host { id },
            reason: None,
            ..
        } = event
            && excluded != Some(id)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// An accepted native producer's identity, never a permission or saved authorization verdict.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Producer {
    Creator { job: Id },
    Native { job: Id },
    OwnerPolicy { job: Id, provider: Id, owner: Id },
}
fn producer(provider: &State, job: &JobRow) -> Producer {
    if creator_authority::is_retained(job) {
        Producer::Creator { job: job.id }
    } else if crate::jobs::lifecycle_commands::applies(job)
        || crate::jobs::recreate::batch_authority::applies(job)
        || job.plan.get("provider_recovery").is_some()
    {
        Producer::Native { job: job.id }
    } else {
        Producer::OwnerPolicy {
            job: job.id,
            provider: provider.id,
            owner: provider.owner_id,
        }
    }
}

/// Restore the accepted reference before choosing a caller. Losing a creator's current source
/// cannot relabel that work as the separate fixed owner-policy producer.
async fn accepted_producer(
    conn: &mut orch_db::Conn,
    provider: &State,
    job: &JobRow,
) -> Result<Option<Producer>, EngineError> {
    let Some(choice) = job.plan.get("pool_choice").filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let choice: Id = serde_json::from_value(choice.clone())?;
    if choice != job.id {
        return Ok(None);
    }
    let Some(row) = orch_db::queries::pool_choices::get(conn, choice).await? else {
        return Ok(None);
    };
    if row.state != "selected"
        || row.source.get("kind") != Some(&json!("provider_placement"))
        || row.source.get("job") != Some(&json!(job.id))
        || row.source.get("job_kind") != Some(&json!(job.kind))
        || row.source.get("provider") != Some(&json!(provider.id))
        || row.source.get("node") != Some(&json!(provider.id))
    {
        return Ok(None);
    }
    let Some(reference) = row.source.get("producer").cloned() else {
        return Ok(None);
    };
    let reference: Producer = serde_json::from_value(reference)?;
    if matches!(&reference, Producer::OwnerPolicy { owner, .. } if *owner != row.actor_id) {
        return Ok(None);
    }
    Ok((reference == producer(provider, job)).then_some(reference))
}

async fn source(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    provider: &State,
    template: &TemplateState,
    job: &JobRow,
    pinned: Option<Id>,
) -> Result<Json, EngineError> {
    let spec = job
        .plan
        .pointer("/provider_recovery/definition")
        .unwrap_or(&template.spec);
    let definition = crate::provider_runtime::definition(conn, provider.id, spec).await?;
    let resources: Option<Json> = setting(
        conn,
        SettingsPlace::Provider(provider.id),
        keys::RUNTIME_RESOURCES,
    )
    .await?;
    let storage: Option<Json> = setting(
        conn,
        SettingsPlace::Provider(provider.id),
        keys::RUNTIME_STORAGE,
    )
    .await?;
    let work = creator_authority::body_hash(&json!({
        "driver":template.driver,
        "definition":definition,
        "resources":crate::places::limits_of(&template.spec, resources.as_ref()),
        "storage":storage,
    }))?;
    Ok(
        json!({"kind":"provider_placement","node":provider.id,"provider":provider.id,
        "job":job.id,"job_kind":job.kind,"producer":producer(provider,job),
        "allocation_operation":principal.caller().admitted_operation(),"owner":provider.owner_id,"home":provider.home_id,
        "template":template.id,"configuration":creator_authority::body_hash(&provider.runtime_config)?,
        "work":work,"retained_host":pinned}),
    )
}

/// The actual admitted Make job owns one durable choice and never redraws on source readback.
pub(super) async fn select(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    provider: &State,
    template: &TemplateState,
    job: &JobRow,
    target: &Target,
    pinned: Option<Id>,
) -> Result<Selected, Stop> {
    target
        .validate(Kind::Host)
        .map_err(EngineError::unreadable)?;
    let source = source(conn, principal, provider, template, job, pinned).await?;
    let choice = Database::new(
        conn,
        principal,
        Facts {
            provider,
            template,
            pinned,
        },
    )
    .reserve(&ChoiceRequest {
        id: job.id,
        target: target.clone(),
        source,
    })
    .await?;
    let Some(Leaf::Host { id }) = choice.leaf.as_ref() else {
        return Err(failed(
            "no_host",
            "The selected host pool has no eligible member.",
        ));
    };
    let leaf = Leaf::Host { id: *id };
    let path = path(&choice.path);
    let selection = orch_authz::pools::Selection {
        leaf: &leaf,
        path: &path,
    };
    let candidate = candidates_scoped(
        conn,
        provider,
        template,
        Some(*id),
        principal,
        Some(selection),
    )
    .await?
    .into_iter()
    .find(|candidate| candidate.host.id == *id && candidate.usable())
    .ok_or_else(|| failed("no_host", "The selected provider host is unavailable."))?;
    let created = placement_bounds(conn, principal, provider, template, *id, Some(selection))
        .await?
        .ok_or_else(|| failed("no_host", "The pool no longer permits this placement."))?;
    Ok(Selected { candidate, created })
}

/// Every first-wire source rebuilds the original creator and rechecks this exact accepted member.
pub(super) fn current<'a>(
    conn: &'a mut orch_db::Conn,
    principal: &'a Principal,
    provider: &'a State,
    template: &'a TemplateState,
    job: &'a JobRow,
    host: Id,
) -> BoxFuture<'a, Result<bool, EngineError>> {
    Box::pin(async move {
        let Some(choice) = job.plan.get("pool_choice").filter(|value| !value.is_null()) else {
            return Ok(false);
        };
        let choice: Id = serde_json::from_value(choice.clone())?;
        let row = orch_db::queries::pool_choices::get(conn, choice)
            .await?
            .ok_or_else(|| EngineError::unreadable("the provider host choice is gone"))?;
        let target: Target = serde_json::from_value(row.target)?;
        let intended: Option<Target> = setting(
            conn,
            SettingsPlace::Provider(provider.id),
            keys::RUNTIME_HOST_ID,
        )
        .await?;
        let leaf: Leaf =
            serde_json::from_value(row.leaf.ok_or_else(|| {
                EngineError::unreadable("the provider host choice has no member")
            })?)?;
        let hops: Vec<Hop> = serde_json::from_value(
            row.path
                .ok_or_else(|| EngineError::unreadable("the provider host choice has no path"))?,
        )?;
        let pinned: Option<Id> =
            serde_json::from_value(job.plan.get("retained_host").cloned().ok_or_else(|| {
                EngineError::unreadable("the provider host source is incomplete")
            })?)?;
        let expected = source(conn, principal, provider, template, job, pinned).await?;
        if intended.as_ref() != Some(&target)
            || choice != job.id
            || row.state != "selected"
            || row.actor_id != principal.user()
            || row.source != expected
            || leaf != (Leaf::Host { id: host })
            || !matches!(
                target,
                Target::Pool {
                    kind: Kind::Host,
                    ..
                }
            )
            || !crate::resource_choice::current_membership(conn, &target, &leaf, &hops).await?
        {
            return Ok(false);
        }
        let path = path(&hops);
        Ok(placement_bounds(
            conn,
            principal,
            provider,
            template,
            host,
            Some(orch_authz::pools::Selection {
                leaf: &leaf,
                path: &path,
            }),
        )
        .await?
        .is_some())
    })
}
