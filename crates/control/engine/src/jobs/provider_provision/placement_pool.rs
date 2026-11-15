//! Provider birth supplies native feasibility to the common choice journal, never a retry loop.
use super::*;
use crate::resource_choice::{
    ChoiceRequest, Hop,
    database::{Database, Requirements},
};
use orch_access_contract::pools::PathMember;
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

async fn source(
    conn: &mut orch_db::Conn,
    provider: &State,
    template: &TemplateState,
    job: Id,
    pinned: Option<Id>,
) -> Result<Json, EngineError> {
    let definition = crate::provider_runtime::definition(conn, provider.id, &template.spec).await?;
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
        "job":job,"template":template.id,"configuration":creator_authority::body_hash(&provider.runtime_config)?,
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
) -> Result<Selected, EngineError> {
    target
        .validate(Kind::Host)
        .map_err(EngineError::unreadable)?;
    let source = source(conn, provider, template, job.id, pinned).await?;
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
        return Err(EngineError::unreadable(
            "the selected host pool has no eligible member",
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
    .ok_or_else(|| EngineError::unreadable("the selected provider host is unavailable"))?;
    let created = placement_bounds(conn, principal, provider, template, *id, Some(selection))
        .await?
        .ok_or_else(|| {
            EngineError::unreadable("the provider host no longer has its bounded pool authority")
        })?;
    Ok(Selected { candidate, created })
}

/// Every first-wire source rebuilds the original creator and rechecks this exact accepted member.
pub(super) async fn current(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    provider: &State,
    template: &TemplateState,
    job: &JobRow,
    host: Id,
) -> Result<bool, EngineError> {
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
    let leaf: Leaf = serde_json::from_value(
        row.leaf
            .ok_or_else(|| EngineError::unreadable("the provider host choice has no member"))?,
    )?;
    let hops: Vec<Hop> = serde_json::from_value(
        row.path
            .ok_or_else(|| EngineError::unreadable("the provider host choice has no path"))?,
    )?;
    let pinned: Option<Id> = serde_json::from_value(
        job.plan
            .get("retained_host")
            .cloned()
            .ok_or_else(|| EngineError::unreadable("the provider host source is incomplete"))?,
    )?;
    let expected = source(conn, provider, template, job.id, pinned).await?;
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
}
