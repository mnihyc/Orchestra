//! Native once-only skill byte supplier and the existing version/draft recording writers.
use super::*;
use crate::{
    key_operations::native::{self, effects},
    registry::Registry,
    routes::{
        nodes::invalid,
        resources::{self, creation_home, skills},
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{
    audit::AuditAction,
    decisions::ObjectRef,
    permissions::{ObjectKind, Permission, ResourceKind},
};
use orch_api_contract::{
    duplicate_sources::{self as sources, Source},
    duplicates::{
        CopySubject, CopySubjectKind, MoveOutBody, MoveOutConfirmation, MoveOutDeleteEffect,
        MoveOutDeletePlan, SkillDraftDuplicate, SkillDraftMoveOut, SkillDraftMoveOutPreview,
        SkillDuplicate, SkillMoveOut, SkillMoveOutPreview,
    },
    operation_sources::Declaration,
    skills::SkillOrigin,
};
use orch_authz::{Caller, Principal};
use orch_db::{
    Conn,
    queries::resources::{grouping, skills as rows},
};
use orch_engine::skills::{
    archive::{self, Limits, SKILL_FILE},
    front_matter, versions,
};
use orch_types::new_id;
use std::{
    marker::PhantomData,
    sync::{Arc, Mutex},
};

trait Mode: Send + Sync + 'static {
    type Body: Clone + Send + Sync + serde::de::DeserializeOwned + 'static;
    const MOVING: bool = false;
    fn request(body: Self::Body) -> (DuplicateBody, Option<MoveOutConfirmation>);

    const DRAFT: bool;
    const ROW: &'static Declaration<Source>;
    const REPLAY: &'static str;
}
struct Published;
struct Draft;
impl Mode for Published {
    type Body = DuplicateBody;
    fn request(body: DuplicateBody) -> (DuplicateBody, Option<MoveOutConfirmation>) {
        (body, None)
    }
    const DRAFT: bool = false;
    const ROW: &'static Declaration<Source> = &sources::SKILL;
    const REPLAY: &'static str = "skill_duplicate";
}
impl Mode for Draft {
    type Body = DuplicateBody;
    fn request(body: DuplicateBody) -> (DuplicateBody, Option<MoveOutConfirmation>) {
        (body, None)
    }
    const DRAFT: bool = true;
    const ROW: &'static Declaration<Source> = &sources::SKILL_DRAFT;
    const REPLAY: &'static str = "skill_draft_duplicate";
}
struct MovePublished;
struct MoveDraft;
impl Mode for MovePublished {
    type Body = MoveOutBody;
    const MOVING: bool = true;
    const DRAFT: bool = false;
    const ROW: &'static Declaration<Source> = &sources::SKILL_MOVE;
    const REPLAY: &'static str = "skill_move_out";
    fn request(body: MoveOutBody) -> (DuplicateBody, Option<MoveOutConfirmation>) {
        (body.copy, Some(body.delete_plan))
    }
}
impl Mode for MoveDraft {
    type Body = MoveOutBody;
    const MOVING: bool = true;
    const DRAFT: bool = true;
    const ROW: &'static Declaration<Source> = &sources::SKILL_DRAFT_MOVE;
    const REPLAY: &'static str = "skill_draft_move_out";
    fn request(body: MoveOutBody) -> (DuplicateBody, Option<MoveOutConfirmation>) {
        (body.copy, Some(body.delete_plan))
    }
}
struct Skills<M: Mode>(PhantomData<M>);
impl<M: Mode> Configuration for Skills<M> {
    const KIND: DuplicateKind = DuplicateKind::Skill;
    const REPLAY: &'static str = M::REPLAY;
    const MOVING: bool = M::MOVING;
}
pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_initial_operation(
        SkillMoveOut,
        effects::initial::<Skills<MovePublished>>,
        effects::prepare::<Skills<MovePublished>>,
        copy::<Skills<MovePublished>>,
    );
    replay::<Skills<MovePublished>, _>(registry, SkillMoveOut);
    registry.on_with_initial_operation(
        SkillDraftMoveOut,
        effects::initial::<Skills<MoveDraft>>,
        effects::prepare::<Skills<MoveDraft>>,
        copy::<Skills<MoveDraft>>,
    );
    replay::<Skills<MoveDraft>, _>(registry, SkillDraftMoveOut);
    registry.on_snapshot_body_read_with_optional_operation(
        SkillMoveOutPreview,
        move_preview_prepare::<Published>,
        move_preview::<Published>,
    );
    registry.on_snapshot_body_read_with_optional_operation(
        SkillDraftMoveOutPreview,
        move_preview_prepare::<Draft>,
        move_preview::<Draft>,
    );

    registry.on_with_initial_operation(
        SkillDuplicate,
        effects::initial::<Skills<Published>>,
        effects::prepare::<Skills<Published>>,
        copy::<Skills<Published>>,
    );
    replay::<Skills<Published>, _>(registry, SkillDuplicate);
    registry.on_with_initial_operation(
        SkillDraftDuplicate,
        effects::initial::<Skills<Draft>>,
        effects::prepare::<Skills<Draft>>,
        copy::<Skills<Draft>>,
    );
    replay::<Skills<Draft>, _>(registry, SkillDraftDuplicate);
}
#[derive(Clone, PartialEq, Eq)]
enum Content {
    Version(Box<rows::VersionRow>),
    Draft {
        revision: i64,
        files: Vec<rows::DraftFileRow>,
    },
}
#[derive(Clone)]
struct Input {
    row: rows::SkillRow,
    content: Content,
    id: Id,
    home: Id,
    name: String,
    limits: Limits,
    body: DuplicateBody,
    confirmation: Option<MoveOutConfirmation>,
}
enum Bytes {
    Version(Box<versions::Prepared>),
    Draft(Vec<rows::DraftFile>),
}
struct Fetched {
    input: Input,
    bytes: Bytes,
}
enum Work {
    Version(Box<versions::Recording>),
    Draft(Box<skills::draft_creation::Prepared>),
}
struct Prepared {
    fetched: Arc<Fetched>,
    work: Mutex<Option<Work>>,
    tags: Vec<Id>,
}
fn permission<M: Mode>() -> Permission {
    if M::DRAFT || M::MOVING {
        Permission::SkillManage
    } else {
        Permission::SkillView
    }
}
fn changed() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::ConflictStaleRevision,
        "the selected skill content changed while it was being copied",
    )
}
fn fault(_error: impl std::fmt::Display) -> ApiError {
    invalid(
        "source",
        "the selected skill content could not be copied; check the source and the destination's skill limits",
    )
}
async fn selected<M: Mode>(conn: &mut Conn, id: Id) -> ApiResult<(rows::SkillRow, Content)> {
    let row = rows::skills(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let content = if M::DRAFT {
        let draft = rows::draft_infos(conn, &[id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        Content::Draft {
            revision: draft.revision,
            files: rows::draft_files(conn, id).await?,
        }
    } else {
        let version = row.latest_version_id.ok_or_else(|| {
            invalid(
                "source",
                "this skill has no published version; duplicate its draft instead",
            )
        })?;
        let version = rows::versions_by_id(conn, &[version])
            .await?
            .pop()
            .filter(|version| version.skill_id == id)
            .ok_or_else(ApiError::not_found)?;
        Content::Version(Box::new(version))
    };
    Ok((row, content))
}
async fn require<M: Mode>(conn: &mut Conn, principal: &Principal, input: &Input) -> ApiResult<()> {
    resources::require(
        conn,
        principal,
        ResourceKind::Skill,
        input.row.id,
        permission::<M>(),
    )
    .await?;
    skills::may_make_at(conn, principal, input.home).await?;
    if selected::<M>(conn, input.row.id).await? != (input.row.clone(), input.content.clone()) {
        return Err(changed());
    }
    copy_to(
        conn,
        principal,
        subject(DuplicateKind::Skill, input.row.id),
        Some(input.home),
    )
    .await?;
    if M::MOVING {
        require_moving(conn, principal, &input.row, input.home).await?;
        let delete = skill_plan(
            &input.row,
            &input.content,
            principal,
            &input.body,
            input.home,
        )?;
        moving::confirm(
            &delete,
            input.confirmation.as_ref().ok_or_else(|| {
                ApiError::invalid(
                    "delete_plan",
                    "the native move has no deletion acknowledgement",
                )
            })?,
        )?;
    }
    Ok(())
}
fn check_files(limits: Limits, files: usize, size: i64) -> ApiResult<()> {
    if files > limits.max_files
        || u64::try_from(size)
            .ok()
            .is_none_or(|size| size > limits.max_bytes)
    {
        return Err(invalid(
            "destination",
            "the copied skill exceeds the destination's current file or byte limit",
        ));
    }
    Ok(())
}
async fn name(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    home: Id,
    body: &DuplicateBody,
    source: &str,
) -> ApiResult<String> {
    if let Some(name) = &body.name {
        if let Some(why) = front_matter::name_fault(name) {
            return Err(invalid("name", why));
        }
        return Ok(name.clone());
    }
    let mut number = 1_u64;
    loop {
        let suffix = if number == 1 {
            "-copy".to_owned()
        } else {
            format!("-copy-{number}")
        };
        let base = source
            .chars()
            .take(front_matter::NAME_MAX_CHARS - suffix.len())
            .collect::<String>();
        let candidate = format!("{}{suffix}", base.trim_end_matches('-'));
        let available = rows::named(conn, home, &candidate).await?.is_none();
        current(ctx, caller)?;
        if available {
            return Ok(candidate);
        }
        number = number
            .checked_add(1)
            .ok_or_else(|| ApiError::internal("the copy name space is exhausted"))?;
    }
}
impl<M: Mode> effects::Initial for Skills<M> {
    type Input = Input;
    type Fetched = Fetched;
    fn declaration(_input: &Input) -> &'static Declaration<Source> {
        M::ROW
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: M::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (body, confirmation) = M::request(body);
            let id = ctx.id("id")?;
            // Establish actual source/destination admission before selecting any protected file manifest.
            let row = rows::skills(conn, &[id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let home = destination(&body, Some(row.home_id)).ok_or_else(|| {
                invalid("destination", "a skill is at home in an org or a project")
            })?;
            let facts = origins(conn, ObjectKind::Skill, id, Some(home)).await?;
            let plan = native::bind(ctx, M::ROW, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            resources::require(conn, &principal, ResourceKind::Skill, id, permission::<M>())
                .await?;
            skills::may_make_at(conn, &principal, home).await?;
            revision(&body, row.revision)?;
            copy_to(
                conn,
                &principal,
                subject(DuplicateKind::Skill, id),
                Some(home),
            )
            .await?;
            let (row, content) = selected::<M>(conn, id).await?;
            if M::MOVING {
                require_moving(conn, &principal, &row, home).await?;
                let delete = skill_plan(&row, &content, &principal, &body, home)?;
                moving::confirm(
                    &delete,
                    confirmation.as_ref().ok_or_else(|| {
                        ApiError::invalid(
                            "delete_plan",
                            "review the native deletion plan before moving",
                        )
                    })?,
                )?;
            }
            let name = name(ctx, conn, principal.caller(), home, &body, &row.name).await?;
            let limits = versions::limits_at(conn, home)
                .await
                .map_err(ApiError::engine)?;
            current(ctx, principal.caller())?;
            Ok((
                Input {
                    row,
                    content,
                    id: new_id(),
                    home,
                    name,
                    limits,
                    body,
                    confirmation,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require::<M>(conn, principal, input))
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            current(ctx, caller)?;
            let bytes = match &input.content {
                Content::Version(version) => {
                    check_files(
                        input.limits,
                        version.files.as_array().map_or(0, Vec::len),
                        version.size,
                    )?;
                    let archive = version.archive_hash()?;
                    let read = ctx
                        .app
                        .blobs
                        .read_whole(
                            &archive,
                            i64::try_from(input.limits.archive_bytes()).unwrap_or(i64::MAX),
                        )
                        .await
                        .map_err(fault)?;
                    current(ctx, caller)?;
                    let limits = input.limits;
                    let name = input.name.clone();
                    let files = tokio::task::spawn_blocking(move || {
                        let mut files = archive::unpack(&read, limits).map_err(fault)?;
                        let skill = files
                            .iter_mut()
                            .find(|file| file.path == SKILL_FILE)
                            .ok_or_else(|| {
                                invalid("source", "the published skill has no skill file")
                            })?;
                        let text = std::str::from_utf8(&skill.data).map_err(fault)?;
                        skill.data = front_matter::with_name(text, &name)
                            .map_err(fault)?
                            .into_bytes();
                        Ok::<_, ApiError>(files)
                    })
                    .await
                    .map_err(ApiError::internal)??;
                    let store = ctx.app.blobs.store();
                    let made = versions::prepare_files(&store, files, input.limits)
                        .await
                        .map_err(fault)?;
                    current(ctx, caller)?;
                    Bytes::Version(Box::new(made))
                }
                Content::Draft { files, .. } => {
                    let size = files
                        .iter()
                        .try_fold(0_i64, |size, file| size.checked_add(file.size))
                        .ok_or_else(|| invalid("source", "the draft's size is invalid"))?;
                    check_files(input.limits, files.len(), size)?;
                    let mut made = Vec::with_capacity(files.len());
                    for file in files {
                        let mut copied = rows::DraftFile {
                            path: file.path.clone(),
                            blob: file.blob_hash()?,
                            size: file.size,
                            executable: file.executable,
                        };
                        if file.path == SKILL_FILE {
                            let read = ctx
                                .app
                                .blobs
                                .read_whole(
                                    &copied.blob,
                                    i64::try_from(input.limits.max_bytes).unwrap_or(i64::MAX),
                                )
                                .await
                                .map_err(fault)?;
                            current(ctx, caller)?;
                            if let Ok(text) = std::str::from_utf8(&read)
                                && front_matter::read(text).is_ok()
                            {
                                let renamed =
                                    front_matter::with_name(text, &input.name).map_err(fault)?;
                                let meta = ctx
                                    .app
                                    .blobs
                                    .put_bytes(bytes::Bytes::from(renamed))
                                    .await
                                    .map_err(ApiError::internal)?;
                                current(ctx, caller)?;
                                copied.blob = meta.hash;
                                copied.size = meta.size;
                            }
                        }
                        made.push(copied);
                    }
                    // Invalid/unfinished draft front matter stays an unpublished draft; no implicit publication.
                    Bytes::Draft(made)
                }
            };
            Ok(Fetched { input, bytes })
        })
    }
}
impl<M: Mode> effects::Effect for Skills<M> {
    type Source = Source;
    type Query = ();
    type Body = M::Body;
    type Value = Prepared;
    type Output = Id;
    fn declaration(_value: &Prepared) -> &'static Declaration<Source> {
        M::ROW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: M::Body,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (body, confirmation) = M::request(body);
            let fetched = effects::initial_value::<Self>(ctx)?;
            let input = &fetched.input;
            if ctx.id("id")? != input.row.id
                || body != input.body
                || confirmation != input.confirmation
            {
                return Err(ApiError::new(
                    orch_api_contract::common::ErrorCode::AuthzDenied,
                    "the native skill copy belongs to another source request",
                ));
            }
            rows::lock(conn, input.row.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if M::DRAFT {
                rows::lock_draft(conn, input.row.id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
            }
            creation_home::lock(conn, Some(input.home)).await?;
            let facts = origins(conn, ObjectKind::Skill, input.row.id, Some(input.home)).await?;
            let plan = native::bind(ctx, M::ROW, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            require::<M>(conn, &principal, input).await?;
            let limits = versions::limits_at(conn, input.home)
                .await
                .map_err(ApiError::engine)?;
            let work = match &fetched.bytes {
                Bytes::Version(prepared) => {
                    check_files(limits, prepared.built.manifest.len(), prepared.built.size)?;
                    Work::Version(Box::new(
                        versions::prepare_record(
                            conn,
                            versions::Target::NewSkill {
                                id: input.id,
                                home: input.home,
                                owner: principal.user(),
                            },
                            prepared,
                            &SkillOrigin::Copy,
                            Some(principal.user()),
                        )
                        .await
                        .map_err(fault)?,
                    ))
                }
                Bytes::Draft(files) => {
                    let size = files
                        .iter()
                        .try_fold(0_i64, |size, file| size.checked_add(file.size))
                        .ok_or_else(|| invalid("source", "the draft's size is invalid"))?;
                    check_files(limits, files.len(), size)?;
                    Work::Draft(Box::new(
                        skills::draft_creation::prepare_files(
                            conn,
                            &principal,
                            input.id,
                            input.home,
                            &input.name,
                            orch_authz::ownership::NewAccess::Defaults,
                            files.clone(),
                        )
                        .await?,
                    ))
                }
            };
            let tags = copy_tags(conn, ResourceKind::Skill, input.row.id, Some(input.home)).await?;
            current(ctx, principal.caller())?;
            Ok((
                Prepared {
                    fetched,
                    work: Mutex::new(Some(work)),
                    tags,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require::<M>(conn, principal, &value.fetched.input).await?;
            let limits = versions::limits_at(conn, value.fetched.input.home)
                .await
                .map_err(ApiError::engine)?;
            match &value.fetched.bytes {
                Bytes::Version(prepared) => {
                    check_files(limits, prepared.built.manifest.len(), prepared.built.size)
                }
                Bytes::Draft(files) => {
                    let size = files
                        .iter()
                        .try_fold(0_i64, |size, file| size.checked_add(file.size))
                        .ok_or_else(|| invalid("source", "the draft's size is invalid"))?;
                    check_files(limits, files.len(), size)
                }
            }
        })
    }
}
impl<M: Mode> effects::Apply for Skills<M> {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Id>>> {
        Box::pin(async move {
            let work = {
                value
                    .work
                    .lock()
                    .map_err(|_| ApiError::internal("the skill recording lock is poisoned"))?
                    .take()
                    .ok_or_else(|| {
                        ApiError::internal("the native skill copy was already recorded")
                    })?
            };
            let input = &value.fetched.input;
            let boundary = if M::MOVING {
                copy_boundary(
                    conn,
                    principal,
                    subject(DuplicateKind::Skill, input.row.id),
                    Some(input.home),
                )
                .await?
            } else {
                None
            };
            let deadline = before.deadline();
            let skill = match work {
                Work::Version(prepared) => {
                    let made = versions::apply_record(
                        conn,
                        Some(principal.user()),
                        *prepared,
                        ctx.app.clock.time(),
                        move || before.check(),
                    )
                    .await?
                    .map_err(fault)?;
                    deadline.check()?;
                    orch_authz::ownership::start_resource(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Skill,
                            id: made.skill.id,
                        },
                        principal.user(),
                        orch_authz::ownership::NewAccess::Defaults,
                        ctx.app.clock.time(),
                    )
                    .await
                    .map_err(ApiError::internal)?;
                    made.skill
                }
                Work::Draft(prepared) => {
                    skills::draft_creation::apply(
                        conn,
                        principal,
                        *prepared,
                        ctx.app.clock.now(),
                        move || before.check(),
                    )
                    .await?
                }
            };
            deadline.check()?;
            if !value.tags.is_empty() {
                grouping::set_tags(conn, ResourceKind::Skill, skill.id, &value.tags, ctx.now)
                    .await?;
                deadline.check()?;
            }
            resources::record(
                ctx,
                conn,
                orch_db::audit::action!(AuditAction::SkillCreated),
                skill.id,
                resources::Home {
                    node: Some(skill.home_id),
                    org: Some(skill.org_id),
                },
                "Duplicated a skill",
                serde_json::json!({"skill":skill.id,"name":skill.name}),
            )
            .await?;
            deadline.check()?;
            if M::MOVING {
                let boundary = boundary.ok_or_else(|| {
                    ApiError::internal("the moving skill source boundary disappeared")
                })?;
                orch_engine::copy_out::record(
                    conn,
                    boundary,
                    orch_db::audit::Actor::User(principal.user()),
                    &crate::sign_in::origin(ctx),
                    skill.id,
                    ctx.now,
                )
                .await
                .map_err(ApiError::engine)?;
                deadline.check()?;
                skills::delete_native(ctx, conn, &input.row).await?;
                deadline.check()?;
            }
            Ok(effects::Outcome::Applied(skill.id))
        })
    }
}

async fn require_moving(
    conn: &mut Conn,
    principal: &Principal,
    row: &rows::SkillRow,
    home: Id,
) -> ApiResult<()> {
    let destination = crate::routes::nodes::placed(conn, home).await?.0.org;
    if destination == row.org_id {
        return Err(invalid(
            "destination",
            "use the existing Move within this organization",
        ));
    }
    if !owns(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Skill,
            id: row.id,
        },
    )
    .await?
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "only an actual owner can move this into another organization",
        ));
    }
    resources::require(
        conn,
        principal,
        ResourceKind::Skill,
        row.id,
        Permission::SkillManage,
    )
    .await
}
fn skill_plan(
    row: &rows::SkillRow,
    content: &Content,
    principal: &Principal,
    body: &DuplicateBody,
    home: Id,
) -> ApiResult<MoveOutDeletePlan> {
    let content = match content {
        Content::Version(version) => {
            serde_json::json!({"version":version.id,"archive":version.archive,"files":version.files,"size":version.size})
        }
        Content::Draft { revision, files } => {
            serde_json::json!({"revision":revision,"files":files.iter().map(|file|
            serde_json::json!({"path":file.path,"blob":file.blob,"size":file.size,"executable":file.executable})).collect::<Vec<_>>()})
        }
    };
    moving::plan(
        CopySubject {
            kind: CopySubjectKind::Skill,
            id: row.id,
        },
        DuplicateDestination::Node { id: home },
        row.revision,
        principal.user(),
        body,
        serde_json::json!({"source_home":row.home_id,"source_org":row.org_id,
            "name":row.name,"origin":row.origin,"content":content}),
        vec![MoveOutDeleteEffect::RemoveSource],
    )
}
struct MovePreview<M: Mode>(PhantomData<M>);
struct PreviewValue {
    row: rows::SkillRow,
    content: Content,
    home: Id,
    plan: MoveOutDeletePlan,
}
fn move_preview_prepare<'a, M: Mode>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: DuplicateBody,
) -> crate::registry::OptionalOperationFuture<'a> {
    native::selected::<MovePreview<M>>(ctx, conn, caller, body, ())
}
async fn move_preview<M: Mode>(
    ctx: Ctx,
    (): (),
    body: DuplicateBody,
) -> ApiResult<MoveOutDeletePlan> {
    native::selected_read::<MovePreview<M>>(ctx, body, ()).await
}
impl<M: Mode> native::Selected for MovePreview<M> {
    type Source = Source;
    type Query = DuplicateBody;
    type Value = PreviewValue;
    type Reply = MoveOutDeletePlan;
    const DECLARATION: &'static Declaration<Source> = if M::DRAFT {
        &sources::SKILL_DRAFT_MOVE_PREVIEW
    } else {
        &sources::SKILL_MOVE_PREVIEW
    };
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        body: &'a DuplicateBody,
    ) -> BoxFuture<'a, ApiResult<(PreviewValue, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::skills(conn, &[ctx.id("id")?])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let home = destination(body, Some(row.home_id)).ok_or_else(|| {
                invalid("destination", "a skill needs an organization or project")
            })?;
            let mut facts = origins(conn, ObjectKind::Skill, row.id, Some(home)).await?;
            facts.push(native::Facts::one(
                Source::PreviewContent,
                orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Skill),
                crate::key_operations::targets::resource_identity(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Skill,
                        id: row.id,
                    },
                )
                .await?,
            ));
            let operation = native::bind(ctx, Self::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &operation).await?;
            require_moving(conn, &principal, &row, home).await?;
            revision(body, row.revision)?;
            skills::may_make_at(conn, &principal, home).await?;
            copy_to(
                conn,
                &principal,
                subject(DuplicateKind::Skill, row.id),
                Some(home),
            )
            .await?;
            let (row, content) = selected::<M>(conn, row.id).await?;
            let plan = skill_plan(&row, &content, &principal, body, home)?;
            Ok((
                PreviewValue {
                    row,
                    content,
                    home,
                    plan,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            ctx.user()?;
            Ok(())
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a PreviewValue,
    ) -> BoxFuture<'a, ApiResult<MoveOutDeletePlan>> {
        Box::pin(async move {
            require_moving(conn, principal, &value.row, value.home).await?;
            skills::may_make_at(conn, principal, value.home).await?;
            if selected::<M>(conn, value.row.id).await?
                != (value.row.clone(), value.content.clone())
            {
                return Err(changed());
            }
            Ok(value.plan.clone())
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a DuplicateBody,
        opened: &'a PreviewValue,
        fresh: &'a PreviewValue,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.plan == fresh.plan) })
    }
}
