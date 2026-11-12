//! Once-only native Skill suppliers, guarded writers and current completed projection.

pub(super) use replay::binding as replay_binding;

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::http_origin;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::HttpIdentityMode;

use orch_engine::commands::CommandAuthority;

use orch_engine::commands::Outgoing;

use orch_engine::operation_scope::continuing;

use orch_engine::operation_scope::http_origin::HttpOrigin;

use orch_engine::operation_scope::skill_creation::Creation;

use orch_engine::operation_scope::skill_creation::Input;

use orch_engine::operation_scope::skill_creation::Intent;

use orch_link_contract::commands::CommandBody;

use std::collections::BTreeMap;

use std::sync::Arc;

use std::sync::Mutex;

use tokio::sync::OnceCell;

use crate::registry::OperationFuture;

use crate::routes::access::operations::NameRoles;

use crate::routes::resources::projection::single;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::Resource;

use orch_api_contract::skill_sources as sources;

use orch_api_contract::skill_sources::Source;

use orch_authz::Caller;
struct Seed {
    input: Input,
    raw: HttpOrigin,
    limits: Limits,
}

struct StageResult {
    shown: String,
    secret: Option<SecretName>,
}

struct Command {
    command: Outgoing,
    authority: CommandAuthority,
}

struct Invocation {
    id: Id,
    body: api::SkillCreateBody,
    seed: OnceCell<Seed>,
    ready: OnceCell<Arc<ReadySource>>,
    url: OnceCell<StageResult>,
    draft_blob: OnceCell<orch_blob_contract::range::BlobMeta>,
    completed: OnceCell<(Completed, Caller)>,
    commands: Mutex<BTreeMap<String, Arc<Command>>>,
}

impl PartialEq for Invocation {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.body == other.body
    }
}

pub(super) struct Opening {
    state: Arc<Invocation>,
    input: Input,
    raw: HttpOrigin,
    limits: Limits,
}

pub(super) fn mapped(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Native(error) => {
            crate::key_operations::snapshot_read::engine_error(error)
        }
        _ => ApiError::new(
            ErrorCode::AuthzDenied,
            "the original Skill creation source is no longer current",
        ),
    }
}

fn intent(source: &SkillSource) -> ApiResult<Intent> {
    Ok(match source {
        SkillSource::New { .. } => Intent::New,
        SkillSource::Upload { blob, .. } => Intent::Upload { blob: *blob },
        SkillSource::Environment {
            environment_id,
            path,
        } => Intent::Environment {
            environment: *environment_id,
            path: paths::clean("from.path", path, true)?,
        },
        SkillSource::Git {
            host_id,
            repository,
            reference,
            folder,
        } => {
            if !orch_settings_contract::registry::is_repository_address(repository) {
                return Err(invalid(
                    "from.repository",
                    "a repository is an https, http or git address without a password",
                ));
            }
            Intent::Git {
                host: *host_id,
                repository: repository.clone(),
                reference: reference.trim().into(),
                folder: folder
                    .as_deref()
                    .map(str::trim)
                    .filter(|folder| !folder.is_empty())
                    .map(|folder| paths::clean("from.folder", folder, false))
                    .transpose()?,
            }
        }
        SkillSource::Url { host_id, url, .. } => Intent::Url {
            host: *host_id,
            address: Hash::of(url.as_bytes()),
            private: source_url::public_url(url).1,
        },
    })
}

impl Opening {
    pub async fn capture(ctx: &Ctx, body: &api::SkillCreateBody) -> ApiResult<Self> {
        let state = match ctx.retained_source::<Invocation>() {
            Ok(state) => state,
            Err(_) => {
                ctx.retain_source(Invocation {
                    id: orch_types::new_id(),
                    body: body.clone(),
                    seed: OnceCell::new(),
                    ready: OnceCell::new(),
                    url: OnceCell::new(),
                    draft_blob: OnceCell::new(),
                    completed: OnceCell::new(),
                    commands: Mutex::new(BTreeMap::new()),
                })?;
                ctx.retained_source::<Invocation>()?
            }
        };
        if state.body != *body {
            return Err(ApiError::internal(
                "the Skill creation changed its original input",
            ));
        }
        let seed = state
            .seed
            .get_or_try_init(|| async {
                let (mut tx, guard) = transaction_guard::begin(ctx).await?;
                let (selected, principal, _plan) =
                    creation_sources::prepare(ctx, &mut tx, guard.principal().caller(), body)
                        .await?;
                selected.require_same_input(body)?;
                let limits = versions::limits_at(&mut tx, body.home_id)
                    .await
                    .map_err(ApiError::engine)?;
                let input = Input::capture(
                    &mut tx,
                    principal.caller(),
                    state.id,
                    body.home_id,
                    intent(&body.from)?,
                    limits,
                )
                .await
                .map_err(ApiError::engine)?;
                let raw = http_origin::capture_operation(ctx, &mut tx, input.origin()).await?;
                let frame = raw
                    .prepare::<Creation>(&mut tx, ctx.app.clock.as_ref(), input.clone())
                    .await
                    .map_err(mapped)?;
                let raw = frame
                    .http_origin()
                    .cloned()
                    .ok_or_else(|| ApiError::internal("the Skill source lost its HTTP identity"))?;
                guard.check_deadline(ctx)?;
                tx.commit().await.map_err(DbError::from)?;
                Ok::<_, ApiError>(Seed { input, raw, limits })
            })
            .await?;
        let opening = Self {
            state: state.clone(),
            input: seed.input.clone(),
            raw: seed.raw.clone(),
            limits: seed.limits,
        };
        opening.check(ctx).await?;
        Ok(opening)
    }
    pub fn id(&self) -> Id {
        self.input.skill()
    }
    pub async fn current(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
    ) -> ApiResult<continuing::Frame<Creation>> {
        let raw = http_origin::current(ctx, conn, &self.raw).await?;
        let identity =
            transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Read).await?;
        let until = ctx
            .publication
            .retained_deadline()?
            .into_iter()
            .chain(identity.expires_at)
            .chain(raw.until())
            .min();
        let raw = self
            .raw
            .with_ceiling(&identity.caller, until)
            .map_err(mapped)?;
        raw.prepare::<Creation>(conn, ctx.app.clock.as_ref(), self.input.clone())
            .await
            .map_err(mapped)
    }
    pub async fn check(&self, ctx: &Ctx) -> ApiResult<()> {
        let mut tx = ctx.app.db.begin().await?;
        let frame = self.current(ctx, &mut tx).await?;
        tx.commit().await.map_err(DbError::from)?;
        frame.check_deadline(ctx.app.clock.as_ref()).map_err(mapped)
    }
    pub async fn rpc<B: CommandBody>(
        &mut self,
        ctx: &Ctx,
        machine: Id,
        body: &B,
    ) -> ApiResult<B::Result> {
        let mut json = serde_json::to_value(body).map_err(ApiError::internal)?;
        json.sort_all_objects();
        let key = format!(
            "{}:{}:{}",
            machine,
            B::KIND.as_str(),
            Hash::of(&serde_json::to_vec(&json).map_err(ApiError::internal)?)
        );
        let existing = self
            .state
            .commands
            .lock()
            .map_err(|_| ApiError::internal("the Skill supplier state is poisoned"))?
            .get(&key)
            .cloned();
        let entry = if let Some(entry) = existing {
            self.check(ctx).await?;
            entry
        } else {
            let mut tx = ctx.app.db.begin().await?;
            let frame = self.current(ctx, &mut tx).await?;
            self.raw = frame
                .http_origin()
                .cloned()
                .ok_or_else(|| ApiError::internal("the Skill command lost its HTTP source"))?;
            let mut command = Outgoing::new(orch_types::new_id(), machine, SerialKey::None, body)
                .map_err(ApiError::internal)?;
            command.deadline = Some(Time::new(
                ctx.app.clock.now() + chrono::Duration::seconds(i64::from(B::KIND.deadline_secs())),
            ));
            let authority = http_origin::bind_source(
                &self.raw,
                &command,
                self.input.command(&command).map_err(ApiError::engine)?,
            )?;
            tx.commit().await.map_err(DbError::from)?;
            frame
                .check_deadline(ctx.app.clock.as_ref())
                .map_err(mapped)?;
            let entry = Arc::new(Command { command, authority });
            self.state
                .commands
                .lock()
                .map_err(|_| ApiError::internal("the Skill supplier state is poisoned"))?
                .insert(key, entry.clone());
            entry
        };
        let dispatcher = ctx.app.link.dispatcher();
        let mut tx = ctx.app.db.begin().await?;
        let frame = self.current(ctx, &mut tx).await?;
        tx.commit().await.map_err(DbError::from)?;
        dispatcher
            .issue_stored_before(
                crate::dispatch::StoredCommand {
                    id: Some(entry.command.id),
                    machine: entry.command.machine,
                    kind: entry.command.kind.as_str().into(),
                    key: entry.command.key.wire(),
                    deadline: entry.command.deadline,
                    body: entry.command.body.clone(),
                    authority: Some(entry.authority.clone()),
                },
                Box::new(|| {
                    frame
                        .check_deadline(ctx.app.clock.as_ref())
                        .map_err(|error| {
                            Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                        })
                }),
            )
            .await
            .map_err(ApiError::internal)?;
        let result = dispatcher.wait_for::<B>(entry.command.id).await;
        let mut tx = ctx.app.db.begin().await?;
        let (raw, current) =
            http_origin::delivery(ctx, &mut tx, &entry.authority, entry.command.id).await?;
        self.raw = raw;
        let frame = self.current(ctx, &mut tx).await?;
        self.raw = frame
            .http_origin()
            .cloned()
            .ok_or_else(|| ApiError::internal("the Skill result lost its HTTP source"))?;
        tx.commit().await.map_err(DbError::from)?;
        http_origin::check(ctx, &current)?;
        frame
            .check_deadline(ctx.app.clock.as_ref())
            .map_err(mapped)?;
        result.map_err(|error| match live::command_failure(error) {
            crate::routes::workspace::problems::Failure::Machine(error) => {
                refused_value("from", "The source could not be read").with("about", [error.detail])
            }
            other => live::refused(other, Doing::Reading),
        })
    }
    pub async fn register(&self, ctx: &Ctx, hash: Hash, size: i64) -> ApiResult<()> {
        let mut tx = ctx.app.db.begin().await?;
        let frame = self.current(ctx, &mut tx).await?;
        frame
            .into_parts()
            .first_effect
            .check(ctx.app.clock.as_ref())
            .map_err(mapped)?;
        storage::register_blob(&mut tx, &hash, size, ctx.app.clock.now()).await?;
        let frame = self.current(ctx, &mut tx).await?;
        tx.commit().await.map_err(DbError::from)?;
        frame.check_deadline(ctx.app.clock.as_ref()).map_err(mapped)
    }
    pub async fn stage_url(
        &mut self,
        ctx: &Ctx,
        url: &str,
    ) -> ApiResult<(String, Option<SecretName>)> {
        let state = self.state.clone();
        let staged = state
            .url
            .get_or_try_init(|| async {
                self.check(ctx).await?;
                let stage = source_url::prepare_stage(self.id(), url, self.input.origin().user())?;
                let mut tx = ctx.app.db.begin().await?;
                let frame = self.current(ctx, &mut tx).await?;
                let by = frame.principal().user();
                let (shown, secret) =
                    source_url::apply_stage(&mut tx, stage, by, ctx.app.clock.now(), || {
                        frame
                            .into_parts()
                            .first_effect
                            .check(ctx.app.clock.as_ref())
                            .map_err(mapped)
                    })
                    .await?;
                let current = self.current(ctx, &mut tx).await?;
                current
                    .check_deadline(ctx.app.clock.as_ref())
                    .map_err(mapped)?;
                tx.commit().await.map_err(DbError::from)?;
                Ok::<_, ApiError>(StageResult { shown, secret })
            })
            .await?;
        self.input = self
            .input
            .with_secret(staged.secret.as_ref().map(|secret| secret.id));
        self.check(ctx).await?;
        Ok((staged.shown.clone(), staged.secret.clone()))
    }
}

impl Opening {
    pub async fn ready(
        &mut self,
        ctx: &Ctx,
        principal: &Principal,
        body: &api::SkillCreateBody,
    ) -> ApiResult<Arc<ReadySource>> {
        let state = self.state.clone();
        let id = self.id();
        let limits = self.limits;
        let ready = state
            .ready
            .get_or_try_init(|| async {
                prepare_with(
                    ctx,
                    principal,
                    id,
                    &body.from,
                    limits,
                    None,
                    Some(&mut *self),
                )
                .await
                .map(Arc::new)
            })
            .await?
            .clone();
        if let Some(secret) = &ready.staged {
            self.input = self.input.with_secret(Some(secret.id));
        }
        self.check(ctx).await?;
        Ok(ready)
    }
}

const ROLES: single::Roles = single::Roles {
    names: NameRoles {
        groups: Some(sources::GROUPS),
        nodes: [
            (ObjectKind::Org, sources::NODES[0]),
            (ObjectKind::Project, sources::NODES[1]),
            (ObjectKind::Topic, sources::NODES[2]),
            (ObjectKind::Thread, sources::NODES[3]),
        ],
        containers: None,
    },
    sources: sources::REFERENCES,
};

pub(super) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: api::SkillCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (_, _, plan) = creation_sources::prepare(ctx, conn, caller, &body).await?;
        Ok(plan)
    })
}

#[derive(Clone)]
struct Completed {
    id: Id,
    version: Option<Id>,
    declaration: &'static Declaration<Source>,
    required: Vec<native::Facts<Source>>,
}

struct Projection;

impl native::resource_reply::ResourceProjection for Projection {
    type Source = Source;
    type Input = Completed;
    type Value = Id;
    const READ_SOURCE: Source = Source::Created;
    const ROLES: &'static single::Roles = &ROLES;
    fn declaration(_ctx: &Ctx, input: &Completed) -> ApiResult<&'static Declaration<Source>> {
        Ok(input.declaration)
    }
    fn load<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Completed,
    ) -> BoxFuture<'a, ApiResult<(Id, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let object = ObjectRef {
                kind: ObjectKind::Skill,
                id: input.id,
            };
            let target = single::subject(conn, object).await?;
            // These required facts identify the accepted writer. Only the new actual Created Read is admitted.
            let mut facts = input.required.clone();
            facts.push(native::Facts::one(
                Source::Created,
                KeyTargetKind::Object(ObjectKind::Skill),
                target,
            ));
            Ok((input.id, facts))
        })
    }
    fn object(id: &Id) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::Skill,
            id: *id,
        }
    }
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: &'a Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Skill,
            *id,
            Permission::SkillView,
        ))
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        old: &'a Id,
        fresh: &'a Id,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old == fresh) })
    }
}

fn receipt(completed: &Completed) -> api::SkillCreateResponse {
    api::SkillCreateResponse::Receipt(api::SkillCreateReceipt {
        projection: ReceiptProjection::Receipt,
        skill_id: completed.id,
        version_id: completed.version,
    })
}

async fn reply(
    ctx: &Ctx,
    accepted: &Caller,
    completed: Completed,
) -> ApiResult<api::SkillCreateResponse> {
    let sparse = receipt(&completed);
    crate::key_operations::publication::receipt(ctx, &sparse)?;
    Ok(
        match native::resource_reply::with_input::<Projection>(ctx, accepted, completed).await? {
            Some(Resource::Skill(full)) => api::SkillCreateResponse::Full(Box::new(full)),
            Some(_) => {
                return Err(ApiError::internal(
                    "a Skill creation returned another resource",
                ));
            }
            None => sparse,
        },
    )
}

pub(super) async fn create(
    ctx: Ctx,
    (): (),
    body: api::SkillCreateBody,
) -> ApiResult<api::SkillCreateResponse> {
    if let Ok(state) = ctx.retained_source::<Invocation>()
        && state.body == body
        && let Some((completed, accepted)) = state.completed.get()
    {
        return reply(&ctx, accepted, completed.clone()).await;
    }
    let mut opening = Opening::capture(&ctx, &body).await?;
    if let SkillSource::New { name, description } = &body.from {
        return draft(&ctx, &body, &mut opening, name, description).await;
    }
    let principal = {
        let mut tx = ctx.app.db.begin().await?;
        let frame = opening.current(&ctx, &mut tx).await?;
        let principal = frame.principal().clone();
        tx.commit().await.map_err(DbError::from)?;
        principal
    };
    let ready = opening.ready(&ctx, &principal, &body).await?;
    let result = async {
        let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
        let id = opening.id();
        let by = guard.principal().user();
        let prepared = versions::prepare_record(
            &mut tx,
            Target::NewSkill {
                id,
                home: body.home_id,
                owner: by,
            },
            &ready.prepared,
            &ready.origin,
            Some(by),
        )
        .await
        .map_err(|error| refused("from", error))?;
        let frame = opening.current(&ctx, &mut tx).await?;
        let limits = frame.value().limits;
        if u64::try_from(ready.prepared.built.size).unwrap_or(u64::MAX) > limits.max_bytes {
            return Err(refused(
                "from",
                SkillError::Refused(SkillFault::TooLarge {
                    limit: limits.max_bytes,
                }),
            ));
        }
        if ready.prepared.built.manifest.len() > limits.max_files {
            return Err(refused(
                "from",
                SkillError::Refused(SkillFault::TooManyFiles {
                    limit: limits.max_files,
                }),
            ));
        }
        let required = frame.value().facts.clone();
        let parts = frame.into_parts();
        let principal = parts.principal;
        let Made { skill, version } = versions::apply_record(
            &mut tx,
            Some(principal.user()),
            prepared,
            ctx.app.clock.time(),
            || {
                parts
                    .first_effect
                    .check(ctx.app.clock.as_ref())
                    .map_err(mapped)
            },
        )
        .await?
        .map_err(|error| refused("from", error))?;
        if ready.origin.can_be_taken_again() {
            rows::set_source_url_secret(&mut tx, id, ready.source_secret).await?;
        }
        ownership::start_resource(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Skill,
                id,
            },
            principal.user(),
            new_access(body.access),
            ctx.app.clock.time(),
        )
        .await
        .map_err(ApiError::internal)?;
        record(
            &ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::SkillCreated),
            id,
            home(&skill),
            "Added a skill",
            json!({"skill":id,"name":skill.name,"version":version.id}),
        )
        .await?;
        orch_db::idempotency::creation::observe_with(
            &mut tx,
            Some(principal.user()),
            "skill",
            id,
            json!({"version":version.id}),
        )
        .await?;
        guard.check_deadline(&ctx)?;
        tx.commit().await.map_err(DbError::from)?;
        let completed = Completed {
            id,
            version: Some(version.id),
            declaration: opening.input.declaration(),
            required,
        };
        Ok::<_, ApiError>((completed, principal.caller().clone()))
    }
    .await;
    match result {
        Ok((completed, accepted)) => {
            opening
                .state
                .completed
                .set((completed.clone(), accepted.clone()))
                .map_err(|_| ApiError::internal("the Skill creation completed twice"))?;
            reply(&ctx, &accepted, completed).await
        }
        Err(error) => {
            abandon_source(&ctx, opening.id(), ready.staged.as_ref()).await;
            Err(error)
        }
    }
}

async fn draft(
    ctx: &Ctx,
    body: &api::SkillCreateBody,
    opening: &mut Opening,
    name: &str,
    description: &str,
) -> ApiResult<api::SkillCreateResponse> {
    let name = name.trim();
    if let Some(why) = front_matter::name_fault(name) {
        return Err(invalid("from.name", why));
    }
    let description = description.trim();
    if description.is_empty() || description.chars().count() > front_matter::DESCRIPTION_MAX_CHARS {
        return Err(invalid(
            "from.description",
            "a description has one to 1,024 characters",
        ));
    }
    opening.check(ctx).await?;
    let meta = *opening
        .state
        .draft_blob
        .get_or_try_init(|| async {
            ctx.app
                .blobs
                .put_bytes(bytes::Bytes::from(new_skill_file(name, description)))
                .await
                .map_err(ApiError::internal)
        })
        .await?;
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let prepared = draft_creation::prepare(
        &mut tx,
        guard.principal(),
        opening.id(),
        body.home_id,
        name,
        new_access(body.access),
        meta,
    )
    .await?;
    let frame = opening.current(ctx, &mut tx).await?;
    let required = frame.value().facts.clone();
    let parts = frame.into_parts();
    let principal = parts.principal;
    let skill = draft_creation::apply(&mut tx, &principal, prepared, ctx.app.clock.now(), || {
        parts
            .first_effect
            .check(ctx.app.clock.as_ref())
            .map_err(mapped)
    })
    .await?;
    record(
        ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::SkillCreated),
        skill.id,
        home(&skill),
        "Added a skill",
        json!({"skill":skill.id,"name":skill.name}),
    )
    .await?;
    orch_db::idempotency::creation::observe_with(
        &mut tx,
        Some(principal.user()),
        "skill",
        skill.id,
        json!({"version":null}),
    )
    .await?;
    guard.check_deadline(ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    let completed = Completed {
        id: skill.id,
        version: None,
        declaration: opening.input.declaration(),
        required,
    };
    let accepted = principal.caller().clone();
    opening
        .state
        .completed
        .set((completed.clone(), accepted.clone()))
        .map_err(|_| ApiError::internal("the Skill draft completed twice"))?;
    reply(ctx, &accepted, completed).await
}

#[path = "creation/replay.rs"]
pub mod replay;
