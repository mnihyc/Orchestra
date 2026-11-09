//! Native configuration preparation, its first database mutation and independent resource readback.

use super::*;
use crate::key_operations::native::effects::{Apply, Effect, FirstEffect, Outcome};
use crate::key_operations::native::resource_reply::ResourceProjection;
use crate::key_operations::{native, targets};
use crate::routes::access::operations::NameRoles;
use crate::routes::resources::projection::single;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::environment_sources::{UPDATE, UpdateSource as Source};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Caller as AuthCaller;

pub(super) struct Update;

pub(super) struct Prepared {
    row: EnvironmentFull,
    body: api::EnvironmentUpdateBody,
    version_consent_changed: bool,
}

impl Prepared {
    fn change(&self) -> Change<'_> {
        Change {
            name: self.body.name.as_deref(),
            config: self.body.config.as_ref(),
            remote_write: self.body.remote_write,
            terminal: self.body.terminal,
            browser: self.body.browser,
            vnc: self.body.vnc,
            home_node_id: self.body.home_node_id,
        }
    }
    fn output(&self, row: &EnvironmentFull, changed: bool, stale: bool) -> Changed {
        Changed {
            environment: row.id,
            revision: row.revision,
            owner: row.owner_id,
            org: row.org_id,
            home: row.home_node_id,
            changed_home: self.body.home_node_id.filter(|_| changed),
            changed,
            stale,
            version_consent_changed: changed && self.version_consent_changed,
            reconcile: changed && (self.body.config.is_some() || self.body.home_node_id.is_some()),
        }
    }
}

pub(super) struct Changed {
    pub environment: Id,
    pub revision: i64,
    pub changed: bool,
    pub stale: bool,
    pub version_consent_changed: bool,
    pub reconcile: bool,
    owner: Id,
    org: Id,
    home: Id,
    changed_home: Option<Id>,
}

async fn environment_fact(conn: &mut Conn, id: Id) -> ApiResult<native::Facts<Source>> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id,
        },
    )
    .await?;
    Ok(native::Facts::one(
        Source::Environment,
        KeyTargetKind::Object(ObjectKind::Environment),
        target,
    ))
}

async fn facts(conn: &mut Conn, id: Id, home: Option<Id>) -> ApiResult<Vec<native::Facts<Source>>> {
    let environment = environment_fact(conn, id).await?;
    let homes = match home {
        Some(id) => vec![(
            KeyTargetKind::Object(ObjectKind::Environment),
            vec![targets::node(conn, id).await?],
        )],
        None => Vec::new(),
    };
    Ok(vec![
        environment,
        native::Facts::collection(Source::Home, homes),
    ])
}

fn safe(caller: &AuthCaller, error: ApiError) -> ApiError {
    if native::modern(caller) {
        authorization::problem_without_content(error)
    } else {
        error
    }
}

async fn require_change(
    conn: &mut Conn,
    principal: &Principal,
    row: &EnvironmentFull,
    home: Option<Id>,
) -> ApiResult<()> {
    require(
        conn,
        principal,
        ResourceKind::Environment,
        row.id,
        Permission::EnvironmentManage,
    )
    .await?;
    if let Some(new_home) = home {
        if !queries::same_tree(conn, new_home, row.home_node_id).await? {
            return Err(invalid(
                "home_node_id",
                "Use Move to another org to copy there and delete here.",
            ));
        }
        let policy = policy_at(conn, new_home).await?;
        authorization::require(
            conn,
            principal,
            new_home,
            PermissionName::OfNode(NodePermission::Interact),
            Act::Use,
            policy,
        )
        .await?;
    }
    Ok(())
}

impl Effect for Update {
    type Source = Source;
    type Query = ();
    type Body = api::EnvironmentUpdateBody;
    type Value = Prepared;
    type Output = Changed;
    fn declaration(_value: &Prepared) -> &'static Declaration<Source> {
        &UPDATE
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a AuthCaller,
        (): (),
        mut body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            body.config = body.config.as_ref().map(registry::canonical_own_values);
            body.name = body
                .name
                .as_deref()
                .map(|name| clean_name("name", name))
                .transpose()?;
            if let Some(config) = &body.config {
                clean_patch_config("config", config)?;
            }
            let id = ctx.id("id")?;
            let row = queries::environments(conn, &[id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let known = vec![environment_fact(conn, id).await?];
            let plan = native::bind_known(ctx, &UPDATE, &known)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            require_change(conn, &principal, &row, None)
                .await
                .map_err(|error| safe(caller, error))?;
            // This is the existing native configuration/runtime serialization row.
            updates::ensure_unheld(conn, id)
                .await
                .map_err(|error| safe(caller, error))?;
            let row = queries::environments(conn, &[id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            body.home_node_id = body.home_node_id.filter(|home| *home != row.home_node_id);
            let known = vec![environment_fact(conn, id).await?];
            let plan = native::bind_known(ctx, &UPDATE, &known)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            require_change(conn, &principal, &row, None)
                .await
                .map_err(|error| safe(caller, error))?;
            let prepared = async {
                if let Some(sources) = &body.data_sources {
                    let mut config = body.config.take().unwrap_or_else(|| row.config.clone());
                    config.as_object_mut().ok_or_else(|| invalid("config", "the environment's own values are an object"))?
                        .insert(registry::keys::ENV_DATA_SOURCES.into(), serde_json::to_value(sources).map_err(ApiError::internal)?);
                    body.config = Some(config);
                }
                let program_report = if body.config.is_some() && let Some(machine) = row.machine_id {
                    queries::machine_program_facts(conn, &[machine]).await?.into_iter().next().map(|(_, report)| report)
                } else { None };
                let mut version_consent_changed = false;
                if let Some(config) = &mut body.config {
                    let report = if row.placement == "connected" && row.template_id.is_none() {
                        program_report.as_ref().unwrap_or(&Json::Null)
                    } else { &Json::Null };
                    version_consent_changed = exec_version::prepare(config, &row.config, report,
                        ctx.user()?.id(), Time::new(ctx.now)).map_err(|why| invalid("config.exec.version_overrides", why))?;
                    if version_consent_changed && ctx.user()?.user.kind != "human" {
                        return Err(ApiError::new(ErrorCode::AuthzDenied,
                            "Only a person managing the environment can change its machine program consent.").with("required", "person"));
                    }
                    clean_config("config", config)?;
                    let sources = orch_driver_contract::runtime::EnvironmentDataSources::from_config(config).map_err(ApiError::internal)?;
                    if sources.borrows_directories() {
                        let machine = orch_db::queries::environment_facts::of(conn, id).await?.and_then(|facts| facts.machine_id)
                            .ok_or_else(|| invalid("data_sources", "choose a machine before choosing an existing directory there"))?;
                        let machine = orch_db::queries::resources::machine(conn, machine).await?.ok_or_else(ApiError::not_found)?;
                        orch_engine::environments::check_machine_data_sources(conn, &machine, &sources).await
                            .map_err(|why| invalid("data_sources", &why))?;
                    }
                }
                if let Some(home) = body.home_node_id
                    && !queries::same_tree(conn, home, row.home_node_id).await? {
                    return Err(invalid("home_node_id", "Use Move to another org to copy there and delete here."));
                }
                let current_facts = facts(conn, id, body.home_node_id).await?;
                let current = native::bind_required(ctx, &UPDATE, &current_facts)?;
                let principal = current.preparation_principal(ctx, conn, caller).await?;
                require_change(conn, &principal, &row, body.home_node_id).await?;
                if let Some(config) = &body.config {
                    let offered = program_report.as_ref().and_then(orch_db::queries::environment_facts::policy_in)
                        .and_then(|policy| texts(policy.get("exec.offers")));
                    let asked = texts(config.get("exec.offers"));
                    if let (Some(offered), Some(asked)) = (offered, asked)
                        && asked.iter().any(|each| !orch_agent_programs::Programs::builtin().offers_include(&offered, each)) {
                        return Err(ApiError::new(ErrorCode::PolicyExecNotOffered,
                            "the machine's local policy does not let its exec node offer that").with("offers", offered));
                    }
                }
                let value = Prepared { row, body, version_consent_changed };
                if value.change() != Change::default() && let Some(name) = &value.body.name
                    && let Some(existing) = queries::named(conn, value.row.owner_id, value.row.org_id, name).await?
                    && existing != id { return Err(taken(Some(existing))); }
                Ok(value)
            }.await.map_err(|error| safe(caller, error))?;
            let facts = facts(conn, id, prepared.body.home_node_id).await?;
            Ok((prepared, facts))
        })
    }

    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_change(conn, principal, &value.row, value.body.home_node_id)
                .await
                .map_err(|error| safe(principal.caller(), error))
        })
    }
}

impl Apply for Update {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Changed>>> {
        Box::pin(async move {
            if value.row.revision != value.body.expected_revision {
                before.check()?;
                return Ok(Outcome::Unchanged(value.output(&value.row, false, true)));
            }
            let change = value.change();
            if change == Change::default() {
                before.check()?;
                return Ok(Outcome::Unchanged(value.output(&value.row, false, false)));
            }
            before.check()?;
            match queries::change(
                conn,
                value.row.id,
                value.body.expected_revision,
                &change,
                ctx.now,
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    return Err(ApiError::new(
                        ErrorCode::ConflictStaleRevision,
                        "the environment was changed since it was read",
                    ));
                }
                Err(DbError::Unique { .. }) => return Err(taken(None)),
                Err(error) => return Err(error.into()),
            }
            let row = &value.row;
            if value.body.config.is_some() || change.home_node_id.is_some() {
                orch_db::queries::resources::mounts::changed(conn, &[row.id], ctx.now).await?;
            }
            let who = actor_name(ctx)?;
            record(ctx, conn, orch_db::audit::action!(AuditAction::EnvironmentUpdated), row.id, home_of(row),
                &format!("{who} changed the environment {}", row.name), json!({"environment":row.id,
                "name":value.body.name,"config":value.body.config.is_some(),"remote_write":value.body.remote_write,
                "terminal":value.body.terminal,"browser":value.body.browser,"vnc":value.body.vnc,"home_node_id":change.home_node_id})).await?;
            let exec_changed = value.body.config.as_ref().is_some_and(|config| {
                config.get("exec.offers") != row.config.get("exec.offers")
                    || config.get("exec.orch_tools") != row.config.get("exec.orch_tools")
                    || value.version_consent_changed
            });
            if exec_changed {
                let mut details = json!({"environment":row.id});
                if value.version_consent_changed {
                    details["version_overrides"] = json!({"before":row.config.get(registry::keys::EXEC_VERSION_OVERRIDES).cloned().unwrap_or_else(|| json!({})),
                        "after":value.body.config.as_ref().and_then(|config| config.get(registry::keys::EXEC_VERSION_OVERRIDES)).cloned().unwrap_or_else(|| json!({}))});
                }
                record(
                    ctx,
                    conn,
                    orch_db::audit::action!(AuditAction::EnvironmentExecChanged),
                    row.id,
                    home_of(row),
                    &format!(
                        "{who} changed what the exec node of the environment {} offers",
                        row.name
                    ),
                    details,
                )
                .await?;
            }
            job_rows::stamp_environment(conn, row.id, ctx.now).await?;
            let current = queries::environments(conn, &[row.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            Ok(Outcome::Applied(value.output(&current, true, false)))
        })
    }
}

impl ResourceProjection for Update {
    type Source = Source;
    type Value = EnvironmentFull;
    type Input = Changed;
    const READ_SOURCE: Source = Source::Environment;
    const ROLES: &'static single::Roles = &single::Roles {
        sources: "environment_update_sources",
        names: NameRoles {
            groups: Some("environment_update_groups"),
            nodes: [
                (ObjectKind::Org, "environment_update_orgs"),
                (ObjectKind::Project, "environment_update_projects"),
                (ObjectKind::Topic, "environment_update_topics"),
                (ObjectKind::Thread, "environment_update_threads"),
            ],
            containers: None,
        },
    };
    fn declaration(_ctx: &Ctx, _input: &Changed) -> ApiResult<&'static Declaration<Source>> {
        Ok(&UPDATE)
    }
    fn load<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Changed,
    ) -> BoxFuture<'a, ApiResult<(EnvironmentFull, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = queries::environments(conn, &[input.environment])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            if row.owner_id != input.owner
                || row.org_id != input.org
                || row.home_node_id != input.home
            {
                return Err(ApiError::not_found());
            }
            let facts = facts(conn, row.id, input.changed_home).await?;
            Ok((row, facts))
        })
    }
    fn object(value: &EnvironmentFull) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::Environment,
            id: value.id,
        }
    }
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a EnvironmentFull,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                ResourceKind::Environment,
                value.id,
                Permission::EnvironmentManage,
            )
            .await
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        opened: &'a EnvironmentFull,
        fresh: &'a EnvironmentFull,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.id == fresh.id
                && opened.owner_id == fresh.owner_id
                && opened.org_id == fresh.org_id
                && opened.home_node_id == fresh.home_node_id
                && opened.placement == fresh.placement
                && opened.template_id == fresh.template_id
                && opened.machine_id == fresh.machine_id)
        })
    }
}
