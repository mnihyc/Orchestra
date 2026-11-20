//! A provider whose physical runtime is lost is explicitly provisioned again under its manager's own destination access.

use crate::{Engine, EngineError, operation_access, updates::UpdateError};
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::loss::OperationAdminGrant;
use orch_authz::Principal;
use orch_db::{
    Conn, DbError,
    queries::{
        jobs,
        resources::{hosting as hosts, provider_runtime as rows},
    },
};
use orch_provider_contract::pools::{Kind, Leaf, Target};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

mod source;
pub use source::Source;

/// Frozen desired runtime, not a promise to recover credentials that existed only on the lost source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// The retained provider.
    pub provider: Id,
    /// Its reviewed revision.
    pub revision: i64,
    /// The actor responsible for destination use.
    pub by: Id,
    /// The actor's narrowed token, if any.
    pub token: Option<Id>,
    /// Reviewed host intent; an accepted native job chooses a pool's physical member once.
    pub host: Target,
    /// Exact reviewed template.
    pub template: Id,
    /// Frozen public template, retaining the old runtime's required egress restriction.
    pub definition: Json,
}
/// A plan's selected recipe and explicit administrative additions.
pub struct Draft {
    /// The desired replacement.
    pub recipe: Recipe,
    /// Grants that confirmation, not preview, makes permanent.
    pub grants: Vec<OperationAdminGrant>,
    /// The readable destination.
    pub host_name: String,
}

/// Read this job's retained physical choice; a reference never grants current pool or Host Use.
pub(crate) async fn selected_host(
    conn: &mut Conn,
    row: &jobs::JobRow,
    target: &Target,
) -> Result<Option<Id>, EngineError> {
    let closed =
        || EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused));
    if let Some(host) = target.single_host() {
        return Ok(Some(host));
    }
    if !matches!(
        target,
        Target::Pool {
            kind: Kind::Host,
            ..
        }
    ) {
        return Err(closed());
    }
    let Some(reference) = row.plan.get("pool_choice").filter(|value| !value.is_null()) else {
        return if row.plan.get("runtime").is_none_or(Json::is_null) {
            Ok(None)
        } else {
            Err(closed())
        };
    };
    let reference: Id = serde_json::from_value(reference.clone()).map_err(|_| closed())?;
    let saved = orch_db::queries::pool_choices::get(conn, reference)
        .await?
        .ok_or_else(closed)?;
    let Leaf::Host { id } =
        serde_json::from_value(saved.leaf.ok_or_else(closed)?).map_err(|_| closed())?
    else {
        return Err(closed());
    };
    if reference != row.id
        || saved.state != "selected"
        || saved.target != serde_json::to_value(target)?
        || saved.source.get("job") != Some(&json!(row.id))
        || saved.source.get("provider") != row.provider_id.as_ref().map(|id| json!(id)).as_ref()
        || row.plan.get("host") != Some(&json!(id))
        || row.created_by != Some(saved.actor_id)
    {
        return Err(closed());
    }
    Ok(Some(id))
}

impl Engine {
    /// The caller must roll back this preview transaction: prospective grants exist only while the ordinary scheduler measures them.
    pub async fn draft_provider_recovery(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        provider: Id,
    ) -> Result<Draft, UpdateError> {
        self.draft_provider_recovery_excluding(conn, principal, provider, None)
            .await
    }

    /// A departing host is excluded from feasibility without drawing or promising a pool member.
    pub async fn draft_provider_recovery_excluding(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        provider: Id,
        excluded: Option<Id>,
    ) -> Result<Draft, UpdateError> {
        let mut grants = Vec::new();
        if let Some(grant) = operation_access::review(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: provider,
            },
            Permission::ProviderManage,
        )
        .await?
        {
            grants.push(grant);
        }
        operation_access::accept(conn, principal, &grants, &orch_db::audit::Origin::default())
            .await?;
        let state = rows::state(conn, provider)
            .await?
            .filter(|state| state.deleted_at.is_none())
            .ok_or(UpdateError::NotFound)?;
        let unavailable: Option<String> =
            sqlx::query_scalar("SELECT runtime_unavailable FROM providers WHERE id=$1")
                .bind(provider)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
        if unavailable.is_none() || rows::live_runtime(conn, provider).await?.is_some() {
            return Err(UpdateError::Blocked(vec![crate::updates::blocker(
                "runtime_not_lost",
                "The provider has no lost runtime to recover",
            )]));
        }
        if orch_settings::provider_home(conn, provider)
            .await
            .map_err(EngineError::from)?
            .directory()
            .is_some()
        {
            return Err(UpdateError::Blocked(vec![crate::updates::blocker(
                "existing_directory_on_source_machine",
                "Choose a new owned harness home before re-creating this lost directory",
            )]));
        }
        let template = get::<Id>(conn, Place::Provider(provider), keys::RUNTIME_TEMPLATE_ID)
            .await
            .map_err(EngineError::from)?
            .unwrap_or(orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID);
        let mut template_row = hosts::template(conn, template)
            .await?
            .filter(|row| row.enabled && row.deleted_at.is_none() && row.kind == "provider")
            .ok_or(UpdateError::NotFound)?;
        let restricted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtimes WHERE provider_id=$1 AND lost_at IS NOT NULL AND network_required='restricted')").bind(provider).fetch_one(&mut *conn).await.map_err(DbError::from)?;
        if restricted && template_row.spec["network"]["mode"] != "none" {
            template_row.spec["network"] = json!({"mode":"bridge","egress":"restricted"});
        }
        if let Some(grant) = operation_access::review(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Template,
                id: template,
            },
            Permission::TemplateUse,
        )
        .await?
        {
            grants.push(grant);
        }
        operation_access::accept(conn, principal, &grants, &orch_db::audit::Origin::default())
            .await?;
        let target = get::<Target>(conn, Place::Provider(provider), keys::RUNTIME_HOST_ID)
            .await
            .map_err(EngineError::from)?;
        if let Some(
            target @ Target::Pool {
                id,
                kind: Kind::Host,
            },
        ) = &target
        {
            if let Some(grant) = operation_access::review(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Pool,
                    id: *id,
                },
                Permission::PoolUse,
            )
            .await?
            {
                grants.push(grant);
                operation_access::accept(
                    conn,
                    principal,
                    &grants,
                    &orch_db::audit::Origin::default(),
                )
                .await?;
            }
            if !crate::jobs::provider_provision::preview_pool(
                conn,
                principal,
                &state,
                &template_row,
                target,
                excluded,
            )
            .await?
            {
                return Err(UpdateError::Blocked(vec![crate::updates::blocker(
                    "no_host",
                    "The selected host pool has no eligible destination",
                )]));
            }
            let row = orch_db::queries::pools::get(conn, *id)
                .await?
                .ok_or(UpdateError::NotFound)?;
            let readable = principal.caller().grant().is_none()
                && orch_authz::resources::check(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Pool,
                        id: *id,
                    },
                    Permission::PoolView,
                    Default::default(),
                )
                .await
                .map_err(EngineError::from)?
                .is_allowed();
            return Ok(Draft {
                recipe: Recipe {
                    provider,
                    revision: state.revision,
                    by: principal.user(),
                    token: principal.caller().grant().and_then(|grant| grant.token()),
                    host: target.clone(),
                    template,
                    definition: template_row.spec,
                },
                grants,
                host_name: if readable { row.name } else { id.to_string() },
            });
        }
        if let Some(target) = &target {
            target.validate(Kind::Host).map_err(UpdateError::Invalid)?;
        }
        let named = target.as_ref().and_then(Target::single_host);
        let candidates = crate::jobs::provider_provision::candidates(
            conn,
            &state,
            &template_row,
            named,
            self.inner.clock.now(),
        )
        .await?;
        let mut permitted = Vec::new();
        let mut host_grants = std::collections::BTreeMap::new();
        for candidate in candidates {
            if excluded == Some(candidate.host.id) {
                continue;
            }
            match operation_access::review(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: candidate.host.id,
                },
                Permission::HostUse,
            )
            .await
            {
                Ok(grant) => {
                    host_grants.insert(candidate.host.id, grant);
                    permitted.push(candidate);
                }
                Err(UpdateError::Denied) => {}
                Err(error) => return Err(error),
            }
        }
        let chosen =
            crate::jobs::provider_provision::choose(&permitted, named).map_err(|failure| {
                UpdateError::Blocked(vec![crate::updates::blocker(
                    &failure.code,
                    failure.message,
                )])
            })?;
        if let Some(Some(grant)) = host_grants.remove(&chosen.host.id) {
            grants.push(grant);
        }
        Ok(Draft {
            recipe: Recipe {
                provider,
                revision: state.revision,
                by: principal.user(),
                token: principal.caller().grant().and_then(|grant| grant.token()),
                host: Target::host(chosen.host.id),
                template,
                definition: template_row.spec,
            },
            grants,
            host_name: chosen.host.name.clone(),
        })
    }
}

/// Queues the ordinary provider provisioning job with a fresh connector state and a frozen destination.
pub async fn queue(
    engine: &Engine,
    conn: &mut Conn,
    principal: &Principal,
    recipe: &Recipe,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<jobs::JobRow, UpdateError> {
    let state = rows::lock(conn, recipe.provider)
        .await?
        .ok_or(UpdateError::NotFound)?;
    if state.revision != recipe.revision || recipe.by != principal.user() {
        return Err(UpdateError::Expired);
    }
    if rows::live_runtime(conn, recipe.provider).await?.is_some() {
        return Err(UpdateError::Blocked(vec![crate::updates::blocker(
            "runtime_changed",
            "The provider already has a runtime",
        )]));
    }
    let row = match crate::jobs::provider_provision::queue(
        conn,
        now,
        recipe.provider,
        crate::jobs::provider_provision::Mode::Make,
        Some(principal.user()),
    )
    .await?
    {
        jobs::provider_jobs::Queued::Made(row) => *row,
        jobs::provider_jobs::Queued::Running(row) => return Err(UpdateError::Held(row.id)),
        jobs::provider_jobs::Queued::NoProvider => return Err(UpdateError::NotFound),
    };
    sqlx::query("UPDATE providers SET runtime_unavailable='Its runtime is being re-created',machine_id=NULL,key_machine_id=CASE WHEN key_machine_id=machine_id THEN NULL ELSE key_machine_id END,status='offline',status_detail='Its runtime is being re-created',runtime_config=runtime_config||jsonb_build_object('runtime.host_id',$2::jsonb),updated_at=$3 WHERE id=$1").bind(recipe.provider).bind(serde_json::to_value(&recipe.host).map_err(EngineError::from)?).bind(now).execute(&mut *conn).await.map_err(DbError::from)?;
    jobs::merge_plan(conn, row.id, &json!({"provider_recovery":recipe}), now).await?;
    let _ = engine;
    Ok(row)
}

/// Every phase retains the actor's own current access, including narrowed tokens and page locks.
pub(crate) async fn guard(
    job: &crate::jobs::runner::Job,
    recipe: &Recipe,
) -> Result<(), crate::jobs::runner::Stop> {
    let mut conn = job.engine().db().acquire().await?;
    let principal =
        crate::jobs::source::load_principal(&mut conn, job, recipe.by, recipe.token).await?;
    let mut needs = vec![
        (
            ObjectKind::Provider,
            recipe.provider,
            Permission::ProviderManage,
        ),
        (
            ObjectKind::Template,
            recipe.template,
            Permission::TemplateUse,
        ),
    ];
    match &recipe.host {
        Target::Single {
            resource: Leaf::Host { id },
        } => needs.push((ObjectKind::Host, *id, Permission::HostUse)),
        Target::Pool {
            id,
            kind: Kind::Host,
        } => {
            let row = job.row().await?;
            if !crate::jobs::lifecycle_commands::environment::present(&row) {
                return Err(crate::jobs::runner::Failure::new(
                    "access.denied",
                    "The original reviewed pool source ended",
                )
                .into());
            }
            needs.push((ObjectKind::Pool, *id, Permission::PoolUse));
        }
        _ => {
            return Err(crate::jobs::runner::Failure::new(
                "access.denied",
                "The reviewed host target has another resource kind",
            )
            .into());
        }
    }
    for (kind, id, permission) in needs {
        if !matches!(
            operation_access::review(&mut conn, &principal, ObjectRef { kind, id }, permission)
                .await,
            Ok(None)
        ) {
            return Err(crate::jobs::runner::Failure::new(
                "access.denied",
                "Current authority no longer permits the reviewed provider recovery",
            )
            .into());
        }
    }
    Ok(())
}
