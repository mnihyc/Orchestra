//! Typed actual native lifecycle recipes and their parent-created children.
use super::*;
use orch_access_contract::{
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::forks::{ForkEnvironment, ForkPlanTarget};
type Need = (ObjectRef, Permission);
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Native {
    Hosted,
    Portable,
    Recovery,
    Switch {
        thread: Id,
    },
    Child {
        parent: Box<JobIdentity>,
        born: Option<Born>,
    },
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Born {
    id: Id,
    owner: Id,
    home: Id,
    template: Option<Id>,
    config: Hash,
}
impl Born {
    async fn load(conn: &mut Conn, id: Id) -> Result<Self, EngineError> {
        let row = envs::environment(conn, id)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or_else(closed)?;
        Ok(Self {
            id,
            owner: row.owner_id,
            home: row.home_node_id,
            template: row.template_id,
            config: crate::jobs::command_identity::hash(&row.config)?,
        })
    }
}
/// Immutable native identity; authority comes only from the enclosing current Source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::jobs::environment_move::preparation) struct JobIdentity {
    id: Id,
    job_kind: String,
    batch: Option<Id>,
    actor: Id,
    environment: Id,
    actual_environment: Option<Id>,
    recipe: Hash,
    native: Native,
    old_runtime: Option<Id>,
    source_machine: Option<Id>,
}
fn needed(kind: ObjectKind, id: Id, permission: Permission) -> Need {
    (ObjectRef { kind, id }, permission)
}
fn recipe_hash(value: &impl Serialize) -> Result<Hash, EngineError> {
    crate::jobs::command_identity::hash(&serde_json::to_value(value)?)
}
impl JobIdentity {
    pub(in crate::jobs::environment_move::preparation) fn capture(
        row: &JobRow,
    ) -> Result<Self, EngineError> {
        let (environment, actor, recipe, native, old_runtime, source_machine) =
            match row.kind.as_str() {
                jobs::kind::ENVIRONMENT_MOVE => {
                    if let Some(portable) = row.plan.get("portable") {
                        let recipe: crate::jobs::machine_move::Recipe =
                            serde_json::from_value(portable.clone())?;
                        (
                            recipe.environment,
                            recipe.by,
                            recipe_hash(&recipe)?,
                            Native::Portable,
                            recipe.old_runtime(),
                            Some(recipe.source_machine),
                        )
                    } else {
                        let recipe: crate::jobs::environment_move::MoveRecipe =
                            serde_json::from_value(row.plan.clone())?;
                        (
                            recipe.replacement.target.id(),
                            recipe.replacement.by,
                            recipe_hash(&recipe)?,
                            Native::Hosted,
                            Some(recipe.replacement.old),
                            recipe.source_machine,
                        )
                    }
                }
                jobs::kind::ENVIRONMENT_PROVISION if row.plan.get("recovery").is_some() => {
                    let recipe: crate::recovery::Recipe =
                        serde_json::from_value(row.plan["recovery"].clone())?;
                    (
                        recipe.environment,
                        recipe.by,
                        recipe_hash(&recipe)?,
                        Native::Recovery,
                        None,
                        None,
                    )
                }
                crate::jobs::fork::KIND => {
                    let recipe: crate::jobs::fork::Recipe =
                        serde_json::from_value(row.plan.clone())?;
                    let (environment, thread) = recipe.lifecycle_source(row)?;
                    (
                        environment,
                        recipe.by,
                        recipe_hash(&(
                            recipe,
                            &row.plan["mount_decisions"],
                            &row.plan["copy_worktree"],
                        ))?,
                        Native::Switch { thread },
                        None,
                        None,
                    )
                }
                _ => return Err(closed()),
            };
        if row.created_by != Some(actor)
            || !matches!(native, Native::Switch { .. }) && row.environment_id != Some(environment)
        {
            return Err(closed());
        }
        Ok(Self {
            id: row.id,
            job_kind: row.kind.clone(),
            batch: row.batch_id,
            actor,
            environment,
            actual_environment: row.environment_id,
            recipe,
            native,
            old_runtime,
            source_machine,
        })
    }
    pub(in crate::jobs::environment_move::preparation) fn actor(&self) -> Id {
        self.actor
    }
    pub(in crate::jobs::environment_move::preparation) fn environment(&self) -> Id {
        self.environment
    }
    pub(in crate::jobs::environment_move::preparation) fn thread(&self) -> Option<Id> {
        match &self.native {
            Native::Switch { thread } => Some(*thread),
            Native::Child { parent, .. } => parent.thread(),
            _ => None,
        }
    }
    pub(in crate::jobs::environment_move::preparation) fn is_recovery(&self) -> bool {
        matches!(self.native, Native::Recovery)
    }
    pub(in crate::jobs::environment_move::preparation) fn is_move(&self) -> bool {
        matches!(self.native, Native::Hosted | Native::Portable)
    }
    pub(in crate::jobs::environment_move::preparation) fn old_runtime(&self) -> Option<Id> {
        self.old_runtime
    }
    pub(in crate::jobs::environment_move::preparation) fn source_machine(&self) -> Option<Id> {
        self.source_machine
    }
    pub(in crate::jobs::environment_move::preparation) fn portable(&self) -> bool {
        matches!(self.native, Native::Portable)
    }
    pub(in crate::jobs::environment_move::preparation) fn matches(
        &self,
        row: &JobRow,
    ) -> Result<bool, EngineError> {
        if row.id != self.id
            || row.kind != self.job_kind
            || row.batch_id != self.batch
            || row.created_by != Some(self.actor)
            || row.environment_id != self.actual_environment
        {
            return Ok(false);
        }
        if matches!(self.native, Native::Child { .. }) {
            return Ok(self.recipe == Self::child_hash(row)?);
        }
        Ok(Self::capture(row)? == *self)
    }
    fn child_hash(row: &JobRow) -> Result<Hash, EngineError> {
        match row.kind.as_str() {
            jobs::kind::ENVIRONMENT_PROVISION => {
                if !matches!(
                    row.plan.get("mode").and_then(Json::as_str),
                    Some("make" | "start")
                ) {
                    return Err(closed());
                }
                recipe_hash(&(
                    row.environment_id,
                    &row.plan["mode"],
                    &row.plan["generation"],
                    &row.plan["by"],
                ))
            }
            jobs::kind::ENVIRONMENT_RECREATE => {
                let recipe: crate::updates::Recipe = serde_json::from_value(row.plan.clone())?;
                if recipe.target
                    != crate::updates::Target::Environment(row.environment_id.ok_or_else(closed)?)
                    || row.created_by != Some(recipe.by)
                {
                    return Err(closed());
                }
                recipe_hash(&recipe)
            }
            _ => Err(closed()),
        }
    }
    pub(in crate::jobs::environment_move::preparation) async fn child(
        &self,
        conn: &mut Conn,
        parent: &JobRow,
        child: &JobRow,
    ) -> Result<Self, EngineError> {
        if !self.matches(parent)?
            || !matches!(self.native, Native::Switch { .. })
            || child.created_by != Some(self.actor)
        {
            return Err(closed());
        }
        let recipe: crate::jobs::fork::Recipe = serde_json::from_value(parent.plan.clone())?;
        let target = child.environment_id.ok_or_else(closed)?;
        let born = if recipe.to_environment == Some(target) {
            if child.kind == jobs::kind::ENVIRONMENT_PROVISION
                && child.plan.get("mode").and_then(Json::as_str) != Some("start")
            {
                return Err(closed());
            }
            None
        } else {
            if parent.plan.get("new_environment") != Some(&serde_json::json!(target))
                || child.kind != jobs::kind::ENVIRONMENT_PROVISION
                || child.plan.get("mode").and_then(Json::as_str) != Some("make")
            {
                return Err(closed());
            }
            let born = Born::load(conn, target).await?;
            let Some(ForkEnvironment::New { new }) = &recipe.request.environment else {
                return Err(closed());
            };
            let home = match new.home_node_id {
                Some(home) => home,
                None => {
                    orch_db::queries::threads::thread(conn, recipe.source_thread)
                        .await?
                        .ok_or_else(closed)?
                        .topic_id
                }
            };
            if born.owner != self.actor
                || born.home != home
                || new.template_id.is_some_and(|id| born.template != Some(id))
            {
                return Err(closed());
            }
            Some(born)
        };
        Ok(Self {
            id: child.id,
            job_kind: child.kind.clone(),
            batch: child.batch_id,
            actor: self.actor,
            environment: self.environment,
            actual_environment: Some(target),
            recipe: Self::child_hash(child)?,
            native: Native::Child {
                parent: Box::new(self.clone()),
                born,
            },
            old_runtime: None,
            source_machine: None,
        })
    }
    pub(in crate::jobs::environment_move::preparation) async fn current(
        &self,
        conn: &mut Conn,
    ) -> Result<bool, EngineError> {
        let Some(row) = jobs::job(conn, self.id).await? else {
            return Ok(false);
        };
        if !row.is_open() || row.cancel_requested() || !self.matches(&row)? {
            return Ok(false);
        }
        if let Native::Child { parent, born } = &self.native {
            let Some(owner) = jobs::job(conn, parent.id).await? else {
                return Ok(false);
            };
            if !owner.is_open() || owner.cancel_requested() || !parent.matches(&owner)? {
                return Ok(false);
            }
            if let Some(born) = born {
                if Born::load(conn, born.id).await? != *born
                    || owner.plan.get("new_environment") != Some(&serde_json::json!(born.id))
                {
                    return Ok(false);
                }
            } else {
                let recipe: crate::jobs::fork::Recipe = serde_json::from_value(owner.plan.clone())?;
                if recipe.to_environment != self.actual_environment {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
    pub(in crate::jobs::environment_move::preparation) async fn owns_recovery_runtime(
        &self,
        conn: &mut Conn,
        current: Option<Id>,
    ) -> Result<bool, EngineError> {
        if !self.is_recovery() {
            return Ok(false);
        }
        let row = jobs::job(conn, self.id).await?.ok_or_else(closed)?;
        if !self.matches(&row)? {
            return Ok(false);
        }
        let recipe: crate::recovery::Recipe = serde_json::from_value(row.plan["recovery"].clone())?;
        let Some(runtime) = current else {
            return Ok(row.plan.get("runtime").is_none());
        };
        if row.plan.get("runtime") != Some(&serde_json::json!(runtime)) {
            return Ok(false);
        }
        let runtime = envs::runtime(conn, runtime).await?.ok_or_else(closed)?;
        Ok(runtime.environment_id == Some(self.environment)
            && runtime.kind == "environment"
            && runtime.host_id == recipe.host
            && runtime.template_id == recipe.template
            && !matches!(runtime.state.as_str(), "retired" | "removed"))
    }
    pub(in crate::jobs::environment_move::preparation) async fn owns_candidate(
        &self,
        conn: &mut Conn,
        runtime: Id,
    ) -> Result<bool, EngineError> {
        if !matches!(self.native, Native::Portable) || self.old_runtime.is_some() {
            return Ok(false);
        }
        let Some(row) = jobs::job(conn, self.id).await? else {
            return Ok(false);
        };
        if !self.matches(&row)? || row.plan.get("runtime") != Some(&serde_json::json!(runtime)) {
            return Ok(false);
        }
        let recipe: crate::jobs::machine_move::Recipe =
            serde_json::from_value(row.plan["portable"].clone())?;
        let Some(candidate) = envs::runtime(conn, runtime).await? else {
            return Ok(false);
        };
        recipe.owns_candidate(&candidate)
    }
    pub(in crate::jobs::environment_move::preparation) async fn wake_matches(
        &self,
        conn: &mut Conn,
        environment: Id,
    ) -> Result<bool, EngineError> {
        if !matches!(self.native, Native::Switch { .. }) {
            return Ok(false);
        }
        let row = jobs::job(conn, self.id).await?.ok_or_else(closed)?;
        if !self.matches(&row)? {
            return Ok(false);
        }
        let recipe: crate::jobs::fork::Recipe = serde_json::from_value(row.plan)?;
        Ok(recipe.to_environment == Some(environment))
    }
    pub(in crate::jobs::environment_move::preparation) async fn destinations(
        &self,
        conn: &mut Conn,
        selected_host: Option<Id>,
    ) -> Result<Vec<Need>, EngineError> {
        let row = jobs::job(conn, self.id).await?.ok_or_else(closed)?;
        if !self.matches(&row)? {
            return Err(closed());
        }
        match &self.native {
            Native::Hosted => {
                let recipe: crate::jobs::environment_move::MoveRecipe =
                    serde_json::from_value(row.plan)?;
                if selected_host.is_some_and(|host| host != recipe.destination) {
                    return Err(closed());
                }
                Ok(vec![
                    needed(ObjectKind::Host, recipe.destination, Permission::HostUse),
                    needed(
                        ObjectKind::Template,
                        recipe.replacement.template,
                        Permission::TemplateUse,
                    ),
                ])
            }
            Native::Portable => {
                let recipe: crate::jobs::machine_move::Recipe =
                    serde_json::from_value(row.plan["portable"].clone())?;
                let needs = recipe.destination_access();
                if selected_host.is_some_and(|host| {
                    !needs
                        .iter()
                        .any(|(o, _)| o.kind == ObjectKind::Host && o.id == host)
                }) {
                    return Err(closed());
                }
                Ok(needs)
            }
            Native::Recovery => {
                let recipe: crate::recovery::Recipe =
                    serde_json::from_value(row.plan["recovery"].clone())?;
                if selected_host.is_some_and(|host| host != recipe.host) {
                    return Err(closed());
                }
                Ok(vec![
                    needed(ObjectKind::Host, recipe.host, Permission::HostUse),
                    needed(
                        ObjectKind::Template,
                        recipe.template,
                        Permission::TemplateUse,
                    ),
                ])
            }
            Native::Switch { .. } => {
                let recipe: crate::jobs::fork::Recipe = serde_json::from_value(row.plan)?;
                switch_destinations(conn, &recipe, selected_host, None).await
            }
            Native::Child { parent, born } => {
                let owner = jobs::job(conn, parent.id).await?.ok_or_else(closed)?;
                if !parent.matches(&owner)? {
                    return Err(closed());
                }
                let recipe: crate::jobs::fork::Recipe = serde_json::from_value(owner.plan)?;
                if born.is_none() {
                    let environment = self.actual_environment.ok_or_else(closed)?;
                    if let Some(host) = selected_host {
                        let runtime = envs::live_runtime(conn, environment)
                            .await?
                            .ok_or_else(closed)?;
                        if runtime.host_id != host {
                            return Err(closed());
                        }
                    }
                    return Ok(vec![needed(
                        ObjectKind::Environment,
                        environment,
                        Permission::EnvironmentManage,
                    )]);
                }
                let native_host = selected_host.or_else(|| {
                    row.plan
                        .get("host")
                        .and_then(Json::as_str)
                        .and_then(|id| id.parse().ok())
                });
                switch_destinations(conn, &recipe, native_host, born.as_ref()).await
            }
        }
    }
}
async fn switch_destinations(
    conn: &mut Conn,
    recipe: &crate::jobs::fork::Recipe,
    selected_host: Option<Id>,
    born: Option<&Born>,
) -> Result<Vec<Need>, EngineError> {
    if let Some(environment) = recipe.to_environment {
        return Ok(vec![needed(
            ObjectKind::Environment,
            environment,
            Permission::EnvironmentManage,
        )]);
    }
    let Some(ForkEnvironment::New { new }) = &recipe.request.environment else {
        return Err(closed());
    };
    let home = match born.map(|value| value.home).or(new.home_node_id) {
        Some(home) => home,
        None => {
            orch_db::queries::threads::thread(conn, recipe.source_thread)
                .await?
                .ok_or_else(closed)?
                .topic_id
        }
    };
    let template = match born.and_then(|value| value.template).or(new.template_id) {
        Some(template) => template,
        None => orch_settings::get::<Id>(
            conn,
            orch_settings::Place::Node(home),
            orch_settings_contract::registry::keys::ENV_TEMPLATE_ID,
        )
        .await?
        .ok_or_else(closed)?,
    };
    let planned = match &recipe.forkplan.to.environment {
        ForkPlanTarget::Made { new } => match &new.place {
            orch_api_contract::environment::Place::Host(host) => host.host.id,
            _ => return Err(closed()),
        },
        _ => return Err(closed()),
    };
    let host = selected_host.unwrap_or(planned);
    if new.host_id.flatten().is_some_and(|named| named != host) {
        return Err(closed());
    }
    let node = orch_db::queries::hierarchy::with_ancestors(conn, &[home])
        .await?
        .into_iter()
        .find(|node| node.id == home)
        .ok_or_else(closed)?;
    let (kind, permission) = match node.kind.as_str() {
        "org" => (ObjectKind::Org, Permission::OrgView),
        "project" => (ObjectKind::Project, Permission::ProjectView),
        "topic" => (ObjectKind::Topic, Permission::TopicView),
        "thread" => (ObjectKind::Thread, Permission::ThreadView),
        _ => return Err(closed()),
    };
    Ok(vec![
        needed(ObjectKind::Host, host, Permission::HostUse),
        needed(ObjectKind::Template, template, Permission::TemplateUse),
        needed(kind, home, permission),
    ])
}
