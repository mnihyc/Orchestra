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
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;

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
    /// Exact reviewed host.
    pub host: Id,
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

impl Engine {
    /// The caller must roll back this preview transaction: prospective grants exist only while the ordinary scheduler measures them.
    pub async fn draft_provider_recovery(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        provider: Id,
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
        let named = get::<Id>(conn, Place::Provider(provider), keys::RUNTIME_HOST_ID)
            .await
            .map_err(EngineError::from)?;
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
                host: chosen.host.id,
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
    sqlx::query("UPDATE providers SET runtime_unavailable='Its runtime is being re-created',machine_id=NULL,key_machine_id=CASE WHEN key_machine_id=machine_id THEN NULL ELSE key_machine_id END,status='offline',status_detail='Its runtime is being re-created',runtime_config=runtime_config||jsonb_build_object('runtime.host_id',$2::uuid::text),updated_at=$3 WHERE id=$1").bind(recipe.provider).bind(recipe.host).bind(now).execute(&mut *conn).await.map_err(DbError::from)?;
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
    for (kind, id, permission) in [
        (
            ObjectKind::Provider,
            recipe.provider,
            Permission::ProviderManage,
        ),
        (ObjectKind::Host, recipe.host, Permission::HostUse),
        (
            ObjectKind::Template,
            recipe.template,
            Permission::TemplateUse,
        ),
    ] {
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
