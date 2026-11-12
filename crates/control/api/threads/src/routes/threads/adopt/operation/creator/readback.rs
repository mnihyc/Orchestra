//! Current readback of the accepted native creator, without replaying birth or history IO.

use super::*;

use crate::key_operations::targets;

use crate::routes::nodes::content_sources;

use crate::routes::nodes::read_sources;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::nodes::Node;

use orch_api_contract::threads::Thread;

use orch_authz::Act;

use orch_authz::Policy;

use orch_db::queries::environments::thread_engine as environments;

use orch_db::queries::threads;

use std::collections::BTreeMap;
pub(crate) struct Input {
    pub prepared: Arc<PreparedAdoption>,
    pub birth: Birth,
    pub created: CreatedThread,
}

enum Read {
    First(Box<Input>),
    Completed(Id),
}

#[derive(Clone, PartialEq)]
struct Subject {
    thread: threads::ThreadRow,
    segment: threads::SegmentRow,
    environment: environments::EnvironmentRow,
}

struct Content;

pub(crate) async fn render(
    ctx: &Ctx,
    accepted: &Caller,
    input: Input,
) -> ApiResult<Option<Thread>> {
    native::reply::render::<Content>(ctx, accepted, Read::First(Box::new(input))).await
}

/// A completed birth uses only its accepted identity and the current native thread reader.
pub(super) async fn completed(ctx: &Ctx, thread: Id) -> ApiResult<Option<Thread>> {
    native::reply::render_completed_replay::<Content>(
        ctx,
        ctx.user()?.id(),
        Read::Completed(thread),
    )
    .await
}

async fn facts(
    conn: &mut Conn,
    input: &Input,
    subject: &Subject,
) -> ApiResult<Vec<native::Facts<Source>>> {
    if input.birth.prepare_source != Some(false)
        || input.birth.worktree.is_some()
        || input.birth.first_input.is_some()
    {
        return Err(ApiError::internal(
            "the accepted adoption has other native creator effects",
        ));
    }
    let mut facts = source::initial_facts(conn, input.prepared.request())
        .await
        .map_err(ApiError::engine)?;
    let environment = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: subject.environment.id,
        },
    )
    .await?;
    let Some(binding) = facts.iter_mut().find(|fact| fact.source == Source::Binding) else {
        return Err(ApiError::internal(
            "the native creator lost its binding facts",
        ));
    };
    binding.groups.push((
        KeyTargetKind::Object(ObjectKind::Environment),
        vec![environment],
    ));
    // These are the real native no-new-environment/no-file/no-first-input decisions.
    for role in [
        Source::NewEnvironmentHomes,
        Source::InitialThreads,
        Source::InitialEnvironments,
        Source::FileEnvironments,
        Source::FileSources,
        Source::MessageChats,
        Source::MessageThreads,
        Source::MessageDocuments,
        Source::MessageEnvironments,
        Source::MessageUploads,
    ] {
        facts.push(native::Facts::collection(role, Vec::new()));
    }
    let gift = input
        .prepared
        .gift_source(&input.birth.agent_access, input.created.thread)
        .map_err(ApiError::engine)?;
    let gift = match gift {
        Some(object) => Some((object.kind, targets::page_object(conn, object).await?)),
        None => None,
    };
    for (kind, role) in [
        (ObjectKind::Project, Source::AgentProjects),
        (ObjectKind::Topic, Source::AgentTopics),
        (ObjectKind::Thread, Source::AgentThreads),
    ] {
        let groups = gift
            .as_ref()
            .filter(|(actual, _)| *actual == kind)
            .map(|(_, target)| vec![(KeyTargetKind::Object(kind), vec![target.clone()])])
            .unwrap_or_default();
        facts.push(native::Facts::collection(role, groups));
    }
    facts.push(native::Facts::one(
        Source::Created,
        KeyTargetKind::Object(ObjectKind::Thread),
        targets::node(conn, input.created.thread).await?,
    ));
    Ok(facts)
}

impl native::reply::Projection for Content {
    type Source = Source;
    type Input = Read;
    type Subject = Subject;
    type Value = content_sources::Prepared;
    type Reply = Thread;
    const READ_SOURCE: Source = Source::Created;
    fn declaration(_: &Read) -> &'static Declaration<Source> {
        &declarations::ADOPT
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Read,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if let Read::Completed(id) = input {
                let thread = threads::thread(conn, *id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                let segment = threads::segment(
                    conn,
                    thread.current_segment_id.ok_or_else(ApiError::not_found)?,
                )
                .await?
                .filter(|segment| segment.thread_id == *id)
                .ok_or_else(ApiError::not_found)?;
                let environment = environments::environment(
                    conn,
                    thread.environment_id.ok_or_else(ApiError::not_found)?,
                )
                .await?
                .ok_or_else(ApiError::not_found)?;
                let facts = vec![native::Facts::one(
                    Source::Created,
                    KeyTargetKind::Object(ObjectKind::Thread),
                    targets::node(conn, *id).await?,
                )];
                return Ok((
                    Subject {
                        thread,
                        segment,
                        environment,
                    },
                    facts,
                ));
            }
            let Read::First(input) = input else {
                return Err(ApiError::internal(
                    "the native adoption reader has no input",
                ));
            };
            let new = input.prepared.new_thread();
            let thread = threads::thread(conn, input.created.thread)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let segment = threads::segment(conn, input.created.segment)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let environment = environments::environment(conn, new.environment)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if thread.topic_id != new.topic
                || thread.agent_id != Some(new.agent)
                || thread.provider_id != Some(new.provider)
                || thread.environment_id != Some(new.environment)
                || thread.workspace_dir != new.directory
                || thread.current_segment_id != Some(segment.id)
                || segment.thread_id != thread.node_id
                || segment.agent_id != new.agent
                || segment.provider_id != new.provider
                || segment.environment_id != new.environment
                || segment.native_session_ref.as_deref()
                    != Some(input.prepared.native_session_ref())
            {
                return Err(ApiError::not_found());
            }
            let subject = Subject {
                thread,
                segment,
                environment,
            };
            let facts = facts(conn, input, &subject).await?;
            Ok((subject, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Read,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            crate::middleware::authorization::require(
                conn,
                principal,
                subject.thread.node_id,
                crate::middleware::authorization::exact(Permission::ThreadView),
                Act::Use,
                Policy::default(),
            )
            .await
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Read,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(content_sources::Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = content_sources::prepare(conn, principal, subject.thread.node_id).await?;
            let objects = value
                .objects
                .iter()
                .map(|&(kind, id)| ObjectRef { kind, id })
                .collect::<Vec<_>>();
            let identities = read_sources::Sources::from_objects(objects.iter().copied())
                .identity_targets(conn)
                .await?;
            let mut groups = BTreeMap::<KeyTargetKind, Vec<_>>::new();
            for object in objects {
                groups
                    .entry(KeyTargetKind::Object(object.kind))
                    .or_default()
                    .push(
                        identities
                            .get(&object)
                            .cloned()
                            .ok_or_else(ApiError::not_found)?,
                    );
            }
            Ok((
                value,
                vec![native::Facts::collection(
                    Source::Header,
                    groups.into_iter().collect(),
                )],
            ))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Read,
        prepared: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Thread>> {
        Box::pin(async move {
            let value = content_sources::render(
                ctx,
                conn,
                principal,
                prepared.subject.thread.node_id,
                &prepared.value,
            )
            .await?;
            match value {
                Node::Thread(thread) => Ok(thread),
                _ => Err(ApiError::not_found()),
            }
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Read,
        opened: &'a native::reply::Prepared<Self>,
        fresh: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject == fresh.subject && opened.value.preserves(&fresh.value))
        })
    }
}
