//! Provider replacements in a Host cascade retain a Provider, never an Environment stand-in.
use super::parent::{Parent, capture_host_parent, current_http, host_principal, raw_error};
use super::*;
use crate::jobs::host_delete::reviewed::ProviderAction;
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission},
};
use orch_db::queries::resources::provider_runtime as providers;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Replace {
        old: Id,
        host: Id,
        selection: Option<Id>,
        template: Id,
        recipe: Hash,
    },
    Make {
        host: Id,
        template: Id,
        recipe: Hash,
    },
}
impl Action {
    fn host(self) -> Id {
        match self {
            Self::Replace { host, .. } | Self::Make { host, .. } => host,
        }
    }
    fn template(self) -> Id {
        match self {
            Self::Replace { template, .. } | Self::Make { template, .. } => template,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    owner: Id,
    home: Id,
    config: Hash,
    runtime: Option<Id>,
}
impl Snapshot {
    async fn load(conn: &mut Conn, provider: Id) -> Result<Self, EngineError> {
        let row = providers::state(conn, provider)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or_else(closed)?;
        Ok(Self {
            owner: row.owner_id,
            home: row.home_id,
            config: crate::jobs::command_identity::hash(&serde_json::json!([
                row.kind,
                row.protocols,
                row.harness_kind,
                row.agent_id,
                row.config,
                row.runtime_config
            ]))?,
            runtime: providers::live_runtime(conn, provider)
                .await?
                .map(|row| row.id),
        })
    }
}
/// The actual reviewed Provider child and original Host confirmation on its queue transaction.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderParentSource {
    parent: Parent,
    http: HttpOrigin,
    job: Id,
    kind: String,
    batch: Option<Id>,
    provider: Id,
    action: Action,
    source: Snapshot,
}
impl std::fmt::Debug for ProviderParentSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProviderParentSource([private])")
    }
}
impl ProviderParentSource {
    pub(super) async fn owns_candidate(
        &self,
        conn: &mut Conn,
        row: &JobRow,
    ) -> Result<bool, EngineError> {
        if !self.matches(row)? || row.plan.get("committed").and_then(Json::as_bool) == Some(true) {
            return Ok(false);
        }
        let Action::Replace {
            old,
            host,
            template,
            ..
        } = self.action
        else {
            return Ok(false);
        };
        let Some(previous) = envs::runtime(conn, old).await? else {
            return Ok(false);
        };
        if previous.provider_id != Some(self.provider)
            || previous.environment_id.is_some()
            || previous.kind != "provider"
            || self.source.runtime != Some(old)
        {
            return Ok(false);
        }
        let Some(next) = row.plan.get("runtime").filter(|value| !value.is_null()) else {
            return Ok(true);
        };
        let next: Id = serde_json::from_value(next.clone())?;
        let Some(runtime) = envs::runtime(conn, next).await? else {
            return Ok(false);
        };
        let published = providers::live_runtime(conn, self.provider)
            .await?
            .map(|runtime| runtime.id);
        Ok(runtime.provider_id == Some(self.provider)
            && runtime.environment_id.is_none()
            && runtime.kind == "provider"
            && runtime.host_id == host
            && runtime.template_id == template
            && runtime.successor_of == Some(old)
            && published != Some(next)
            && self.source.runtime == Some(old))
    }
    pub(super) fn origin(&self) -> &HttpOrigin {
        &self.http
    }
    pub(super) fn operation(&self) -> &'static str {
        "HostDeleteCreate"
    }
    pub(super) async fn current_http(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<HttpOrigin, EngineError> {
        current_http(self.parent, &self.http, conn, clock).await
    }
    pub(super) async fn capture(
        conn: &mut Conn,
        parent: &JobRow,
        child: &JobRow,
        reviewed: &ProviderAction,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        // Leave is the Host's own retirement, not a Provider-management child.
        if matches!(reviewed, ProviderAction::Leave { .. }) {
            return Err(closed());
        }
        let (root, http) = capture_host_parent(conn, parent, child, clock).await?;
        let (provider, actor, action) = match reviewed {
            ProviderAction::Replace { recipe, host } => {
                let crate::updates::Target::Provider(provider) = recipe.target else {
                    return Err(closed());
                };
                if recipe.host.is_some_and(|selected| selected != *host) {
                    return Err(closed());
                }
                (
                    provider,
                    recipe.by,
                    Action::Replace {
                        old: recipe.old,
                        host: *host,
                        selection: recipe.host,
                        template: recipe.template,
                        recipe: crate::jobs::command_identity::hash(&serde_json::to_value(
                            recipe,
                        )?)?,
                    },
                )
            }
            ProviderAction::Make { recipe } => (
                recipe.provider,
                recipe.by,
                Action::Make {
                    host: recipe.host,
                    template: recipe.template,
                    recipe: crate::jobs::command_identity::hash(&serde_json::to_value(recipe)?)?,
                },
            ),
            ProviderAction::Leave { .. } => return Err(closed()),
        };
        if actor != http.origin().user() {
            return Err(closed());
        }
        let source = Snapshot::load(conn, provider).await?;
        let value = Self {
            parent: root,
            http,
            job: child.id,
            kind: child.kind.clone(),
            batch: child.batch_id,
            provider,
            action,
            source,
        };
        if !value.matches(child)? {
            return Err(closed());
        }
        if let Action::Replace { old, .. } = action {
            if value.source.runtime != Some(old) {
                return Err(closed());
            }
        } else if value.source.runtime.is_some() {
            return Err(closed());
        }
        Ok(value)
    }
    pub(super) fn matches(&self, row: &JobRow) -> Result<bool, EngineError> {
        if !matches!(self.parent, Parent::Host { .. })
            || row.id != self.job
            || row.kind != self.kind
            || row.batch_id != self.batch
            || row.provider_id != Some(self.provider)
            || row.environment_id.is_some()
            || row.created_by != Some(self.http.origin().user())
            || self.http.origin().operation() != self.operation()
        {
            return Ok(false);
        }
        Ok(match self.action {
            Action::Replace {
                old,
                host,
                selection,
                template,
                recipe: hash,
            } => {
                if row.kind != jobs::kind::PROVIDER_RECREATE {
                    return Ok(false);
                }
                let mut recipe: crate::updates::Recipe = serde_json::from_value(row.plan.clone())?;
                if recipe.host != selection
                    && !(row
                        .plan
                        .get("runtime")
                        .is_some_and(|value| !value.is_null())
                        && recipe.host == Some(host))
                {
                    return Ok(false);
                }
                recipe.host = selection;
                recipe.target == crate::updates::Target::Provider(self.provider)
                    && recipe.by == self.http.origin().user()
                    && recipe.old == old
                    && recipe.template == template
                    && recipe.host.is_none_or(|selected| selected == host)
                    && crate::jobs::command_identity::hash(&serde_json::to_value(&recipe)?)? == hash
            }
            Action::Make {
                host,
                template,
                recipe: hash,
            } => {
                if row.kind != jobs::kind::PROVIDER_PROVISION
                    || row.plan.get("mode").and_then(Json::as_str) != Some("make")
                {
                    return Ok(false);
                }
                let recipe: crate::provider_recovery::Recipe =
                    serde_json::from_value(row.plan["provider_recovery"].clone())?;
                recipe.provider == self.provider
                    && recipe.by == self.http.origin().user()
                    && recipe.host == host
                    && recipe.template == template
                    && crate::jobs::command_identity::hash(&serde_json::to_value(&recipe)?)? == hash
            }
        })
    }
    pub(super) async fn lock_source(
        &self,
        conn: &mut Conn,
        row: &JobRow,
    ) -> Result<(), EngineError> {
        if !self.matches(row)? {
            return Err(closed());
        }
        self.http.lock_identity(conn).await?;
        let Parent::Host { job, host, .. } = self.parent else {
            return Err(closed());
        };
        sqlx::query("SELECT id FROM jobs WHERE id=$1 FOR SHARE")
            .bind(job)
            .execute(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
        orch_db::queries::resources::lock_resource(conn, "host", host).await?;
        providers::lock(conn, self.provider)
            .await?
            .ok_or_else(closed)?;
        Ok(())
    }
    async fn physical(&self, conn: &mut Conn, row: &JobRow) -> Result<(), EngineError> {
        let current = Snapshot::load(conn, self.provider).await?;
        if current.owner != self.source.owner
            || current.home != self.source.home
            || current.config != self.source.config
        {
            return Err(closed());
        }
        if current.runtime != self.source.runtime {
            let Some(id) = current.runtime else {
                return Err(closed());
            };
            if row.plan.get("runtime") != Some(&serde_json::json!(id)) {
                return Err(closed());
            }
            let runtime = envs::runtime(conn, id).await?.ok_or_else(closed)?;
            if runtime.provider_id != Some(self.provider)
                || runtime.environment_id.is_some()
                || runtime.kind != "provider"
                || runtime.host_id != self.action.host()
                || runtime.template_id != self.action.template()
                || matches!(runtime.state.as_str(), "retired" | "removed")
            {
                return Err(closed());
            }
            match self.action {
                Action::Make { .. } if self.source.runtime.is_none() => {}
                Action::Replace { old, .. }
                    if self.source.runtime == Some(old)
                        && (runtime.successor_of == Some(old)
                            || row.plan.get("committed").and_then(Json::as_bool) == Some(true)) => {
                }
                _ => return Err(closed()),
            }
        }
        // A recipe without an explicit host reads this same current saved native choice.
        let current = providers::state(conn, self.provider)
            .await?
            .ok_or_else(closed)?;
        if current
            .runtime_config
            .get("runtime.host_id")
            .and_then(Json::as_str)
            .and_then(|value| value.parse::<Id>().ok())
            != Some(self.action.host())
        {
            return Err(closed());
        }
        Ok(())
    }
    pub(super) async fn principal(
        &self,
        conn: &mut Conn,
        row: &JobRow,
        clock: &dyn Clock,
        selection: Selection,
    ) -> Result<orch_authz::Principal, EngineError> {
        if !self.matches(row)? || !row.is_open() || row.cancel_requested() {
            return Err(closed());
        }
        match selection {
            Selection::Current => {}
            Selection::Host(host) if host == self.action.host() => {}
            _ => return Err(closed()),
        }
        self.physical(conn, row).await?;
        let http = current_http(self.parent, &self.http, conn, clock).await?;
        let raw = http.current(conn, clock).await.map_err(raw_error)?;
        let needs = [
            (
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: self.action.host(),
                },
                Permission::HostUse,
            ),
            (
                ObjectRef {
                    kind: ObjectKind::Template,
                    id: self.action.template(),
                },
                Permission::TemplateUse,
            ),
        ];
        let principal = host_principal(
            conn,
            self.parent,
            &raw,
            http.session(),
            vec![ObjectRef {
                kind: ObjectKind::Provider,
                id: self.provider,
            }],
            &needs,
            clock,
        )
        .await?;
        // The provider's own placement eligibility remains an independent native prerequisite.
        // It never supplies the requester's operation or destination permission.
        if !crate::updates::provider_owner_host_usable(
            conn,
            self.provider,
            self.action.host(),
            clock.time(),
        )
        .await?
        {
            return Err(closed());
        }
        raw.check(clock).map_err(raw_error)?;
        if !principal.caller().key_deadline_allows(clock) {
            return Err(closed());
        }
        Ok(principal)
    }
}
