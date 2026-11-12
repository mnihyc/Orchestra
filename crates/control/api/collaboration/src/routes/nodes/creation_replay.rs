//! Native node-creation identities reuse their original current content and metadata cuts.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::idempotency::Creation;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind::Object;

use orch_access_contract::permissions::ObjectKind as K;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::nodes::ProjectCreateBody;

use orch_api_contract::nodes::ProjectCreateResponse;

use orch_api_contract::nodes::ThreadCreateReceipt;

use orch_api_contract::nodes::TopicCreateBody;

use orch_api_contract::nodes::TopicCreateReply;

use orch_api_contract::nodes::TopicWithThread;

use orch_api_contract::operation_sources::Binding;

use orch_api_contract::operation_sources::Cardinality;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::TargetResolver as Target;

use orch_api_contract::threads::Thread;

use orch_db::idempotency::creation::Created;

use std::collections::BTreeSet;
const fn collection(source: &'static str, usage: FacetUsage) -> Binding<&'static str> {
    Binding {
        source,
        target: Target::ResolvedCollection(source),
        usage,
        cardinality: Cardinality::Collection,
    }
}

const fn one(source: &'static str, target: Target, usage: FacetUsage) -> Binding<&'static str> {
    Binding {
        source,
        target,
        usage,
        cardinality: Cardinality::One,
    }
}

const PROJECT: &[Binding<&str>] = &[
    one("parent", Target::PermissionObject, FacetUsage::Required),
    one(
        "created",
        Target::Resolved("created_resource"),
        FacetUsage::ResponseContent,
    ),
    collection("created_names_orgs", FacetUsage::ResponseContent),
    collection("created_names_projects", FacetUsage::ResponseContent),
];

const PROJECT_ROOT: Declaration<&str> = Declaration {
    operation: "ProjectCreate",
    bindings: PROJECT,
};

const PROJECT_CHILD: Declaration<&str> = Declaration {
    operation: "ProjectChildCreate",
    bindings: PROJECT,
};

const TOPIC: Declaration<&str> = Declaration {
    operation: "TopicCreate.create",
    bindings: &[
        one("parent", Target::PermissionObject, FacetUsage::Required),
        one(
            "created",
            Target::Resolved("created_resource"),
            FacetUsage::ResponseContent,
        ),
        collection("created_names_orgs", FacetUsage::ResponseContent),
        collection("created_names_projects", FacetUsage::ResponseContent),
        collection("created_environments", FacetUsage::ResponseContent),
    ],
};

const COMPOUND: Declaration<&str> = Declaration {
    operation: "TopicCreate.with_thread",
    bindings: &[
        one("parent", Target::PermissionObject, FacetUsage::Required),
        one(
            "thread_destination",
            Target::Resolved("first_thread_destination"),
            FacetUsage::Required,
        ),
        one(
            "created",
            Target::Resolved("created_resources"),
            FacetUsage::ResponseContent,
        ),
        one(
            "thread",
            Target::Resolved("created_thread"),
            FacetUsage::ResponseContent,
        ),
        collection("creator_new_environment_homes", FacetUsage::Required),
        collection("creator_initial_file_threads", FacetUsage::Required),
        collection("creator_initial_file_environments", FacetUsage::Required),
        collection("creator_file_environments", FacetUsage::Required),
        collection("creator_reader_environments", FacetUsage::Required),
        collection("creator_file_sources", FacetUsage::Required),
        collection("creator_agent_projects", FacetUsage::Required),
        collection("creator_agent_topics", FacetUsage::Required),
        collection("creator_agent_threads", FacetUsage::Required),
        collection("message_source_chats", FacetUsage::Required),
        collection("message_source_threads", FacetUsage::Required),
        collection("message_source_documents", FacetUsage::Required),
        collection("message_source_environments", FacetUsage::Required),
        collection("message_retained_uploads", FacetUsage::Required),
        collection("creator_names_orgs", FacetUsage::ResponseContent),
        collection("creator_names_projects", FacetUsage::ResponseContent),
        collection("creator_names_topics", FacetUsage::ResponseContent),
        collection("creator_environments", FacetUsage::ResponseContent),
    ],
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Project,
    Child,
    Topic,
    Compound,
}

impl Mode {
    fn row(self) -> &'static Declaration<&'static str> {
        match self {
            Self::Project => &PROJECT_ROOT,
            Self::Child => &PROJECT_CHILD,
            Self::Topic => &TOPIC,
            Self::Compound => &COMPOUND,
        }
    }
    fn kind(self) -> K {
        match self {
            Self::Project | Self::Child => K::Project,
            Self::Topic | Self::Compound => K::Topic,
        }
    }
    fn names(self) -> &'static [(&'static str, K)] {
        match self {
            Self::Compound => &[
                ("creator_names_orgs", K::Org),
                ("creator_names_projects", K::Project),
                ("creator_names_topics", K::Topic),
            ],
            _ => &[
                ("created_names_orgs", K::Org),
                ("created_names_projects", K::Project),
            ],
        }
    }
}

struct Input {
    id: Id,
    thread: Option<Id>,
    mode: Mode,
}

struct Subject {
    parent: Option<Id>,
    child_topic: Option<Id>,
    environment: Option<Id>,
}

struct Value {
    names: BTreeSet<Id>,
    root: content_sources::Prepared,
    child: Option<content_sources::Prepared>,
}

struct Content;

struct Rendered {
    node: Node,
    thread: Option<Thread>,
}

pub(super) fn project<const CHILD: bool>() -> Creation<(), ProjectCreateBody, ProjectCreateResponse>
{
    Creation {
        kind: "project",
        replay: project_replay::<CHILD>,
        identity: project_identity,
    }
}

pub(super) fn topic() -> Creation<(), TopicCreateBody, TopicCreateReply> {
    Creation {
        kind: "topic",
        replay: topic_replay,
        identity: topic_identity,
    }
}

fn project_identity(value: &ProjectCreateResponse) -> ApiResult<Created> {
    let id = match value {
        ProjectCreateResponse::Full(value) => value.base.id,
        ProjectCreateResponse::Receipt(value) => value.node_id,
    };
    Ok(Created {
        kind: "project".into(),
        id,
        data: Json::Null,
    })
}

fn topic_identity(value: &TopicCreateReply) -> ApiResult<Created> {
    let (id, data) = match value {
        TopicCreateReply::Topic(value) => (value.base.id, Json::Null),
        TopicCreateReply::Receipt(value) => (value.node_id, Json::Null),
        TopicCreateReply::WithThread(value) => {
            (value.topic.base.id, json!({"thread":value.thread.base.id}))
        }
        TopicCreateReply::ThreadReceipt(value) => (
            value.topic_id,
            json!({"thread":value.thread_id,"input":value.input_id}),
        ),
    };
    Ok(Created {
        kind: "topic".into(),
        id,
        data,
    })
}

fn unavailable(error: &ApiError) -> bool {
    matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied)
}

fn project_replay<const CHILD: bool>(
    ctx: Ctx,
    (): (),
    _body: ProjectCreateBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<ProjectCreateResponse>> {
    Box::pin(async move {
        let receipt = ProjectCreateResponse::Receipt(creation_operation::receipt(
            created.id,
            NodeKind::Project,
        ));
        publication::receipt(&ctx, &receipt)?;
        let input = Input {
            id: created.id,
            thread: None,
            mode: if CHILD { Mode::Child } else { Mode::Project },
        };
        match reply::render_completed_replay::<Content>(&ctx, ctx.user()?.id(), input).await {
            Ok(Some(Rendered {
                node: Node::Project(value),
                ..
            })) => Ok(ProjectCreateResponse::Full(Box::new(value))),
            Ok(Some(_)) => Err(ApiError::internal(
                "the created project changed native kind",
            )),
            Ok(None) => Ok(receipt),
            Err(error) if unavailable(&error) => Ok(receipt),
            Err(error) => Err(error),
        }
    })
}

fn topic_replay(
    ctx: Ctx,
    (): (),
    body: TopicCreateBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<TopicCreateReply>> {
    Box::pin(async move {
        let thread: Option<Id> = created
            .data
            .get("thread")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(ApiError::internal)?;
        let receipt = if body.first_thread.is_some() {
            TopicCreateReply::ThreadReceipt(ThreadCreateReceipt {
                projection: ReceiptProjection::Receipt,
                topic_id: created.id,
                thread_id: thread.ok_or_else(|| {
                    ApiError::internal("the compound creation has no accepted Thread identity")
                })?,
                input_id: created
                    .data
                    .get("input")
                    .cloned()
                    .map(serde_json::from_value::<Option<Id>>)
                    .transpose()
                    .map_err(ApiError::internal)?
                    .flatten(),
            })
        } else {
            TopicCreateReply::Receipt(creation_operation::receipt(created.id, NodeKind::Topic))
        };
        publication::receipt(&ctx, &receipt)?;
        let input = Input {
            id: created.id,
            thread,
            mode: if body.first_thread.is_some() {
                Mode::Compound
            } else {
                Mode::Topic
            },
        };
        match reply::render_completed_replay::<Content>(&ctx, ctx.user()?.id(), input).await {
            Ok(Some(Rendered {
                node: Node::Topic(topic),
                thread: Some(thread),
            })) => Ok(TopicCreateReply::WithThread(TopicWithThread {
                topic,
                thread,
            })),
            Ok(Some(Rendered {
                node: Node::Topic(topic),
                thread: None,
            })) => Ok(TopicCreateReply::Topic(topic)),
            Ok(Some(_)) => Err(ApiError::internal("the created topic changed native kind")),
            Ok(None) => Ok(receipt),
            Err(error) if unavailable(&error) => Ok(receipt),
            Err(error) => Err(error),
        }
    })
}

impl reply::Projection for Content {
    type Source = &'static str;
    type Input = Input;
    type Subject = Subject;
    type Value = Value;
    type Reply = Rendered;
    const READ_SOURCE: &'static str = "created";
    fn declaration(input: &Input) -> &'static Declaration<Self::Source> {
        input.mode.row()
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Self::Source>>)>> {
        Box::pin(async move {
            let row = hierarchy::node(conn, input.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let target = targets::node(conn, input.id).await?;
            if target.object.kind != Object(input.mode.kind()) {
                return Err(ApiError::not_found());
            }
            let mut facts = vec![native::Facts::one(
                "created",
                Object(input.mode.kind()),
                target,
            )];
            let child = if let Some(id) = input.thread {
                let row = orch_db::queries::threads::thread(conn, id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                facts.push(native::Facts::one(
                    "thread",
                    Object(K::Thread),
                    targets::node(conn, id).await?,
                ));
                Some(row)
            } else {
                None
            };
            Ok((
                Subject {
                    parent: row.parent_id,
                    child_topic: child.as_ref().map(|row| row.topic_id),
                    environment: child.and_then(|row| row.environment_id),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require(
                conn,
                principal,
                input.id,
                authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await?;
            if let Some(id) = input.thread {
                authorization::require(
                    conn,
                    principal,
                    id,
                    authorization::VIEW,
                    Act::Use,
                    Policy::default(),
                )
                .await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Self::Source>>)>> {
        Box::pin(async move {
            let root = targets::node(conn, input.id).await?;
            let mut primary = vec![native::Facts::one(
                "created",
                Object(input.mode.kind()),
                root.clone(),
            )];
            if let Some(id) = input.thread {
                primary.push(native::Facts::one(
                    "thread",
                    Object(K::Thread),
                    targets::node(conn, id).await?,
                ));
            }
            let known = native::bind_known(ctx, input.mode.row(), &primary)?;
            let mut names = BTreeSet::from([input.id]);
            if let Some(id) = input.thread {
                names.insert(id);
            }
            let child_target = if let Some(id) = input.thread {
                Some(targets::node(conn, id).await?)
            } else {
                None
            };
            let mut facts = Vec::new();
            for &(role, kind) in input.mode.names() {
                let mut selected = Vec::new();
                let mut seen = BTreeSet::new();
                for target in std::iter::once(&root).chain(child_target.iter()) {
                    for ancestor in &target.node_ancestors {
                        if ancestor.kind != Object(kind) {
                            continue;
                        }
                        let id = ancestor.id.ok_or_else(ApiError::not_found)?;
                        if !seen.insert(id) {
                            continue;
                        }
                        let target = targets::node(conn, id).await?;
                        if !native::modern(principal.caller())
                            || native::permits_usage(
                                &known,
                                input.mode.row(),
                                principal.caller(),
                                &role,
                                FacetUsage::ResponseContent,
                                Object(kind),
                                &target,
                            )?
                        {
                            names.insert(id);
                            selected.push(target);
                        }
                    }
                }
                facts.push(native::Facts::collection(
                    role,
                    if selected.is_empty() {
                        Vec::new()
                    } else {
                        vec![(Object(kind), selected)]
                    },
                ));
            }
            let mut environments = Vec::new();
            if input.mode == Mode::Topic {
                let sources = crate::routes::environments::reading::topic_summary_sources(
                    conn,
                    principal,
                    &[input.id],
                )
                .await?;
                for (_, id) in sources.bindings {
                    environments.push(
                        targets::page_object(
                            conn,
                            ObjectRef {
                                kind: K::Environment,
                                id,
                            },
                        )
                        .await?,
                    );
                }
            } else if let Some(id) = subject.environment {
                environments.push(
                    targets::page_object(
                        conn,
                        ObjectRef {
                            kind: K::Environment,
                            id,
                        },
                    )
                    .await?,
                );
            }
            if matches!(input.mode, Mode::Topic | Mode::Compound) {
                facts.push(native::Facts::collection(
                    if input.mode == Mode::Topic {
                        "created_environments"
                    } else {
                        "creator_environments"
                    },
                    if environments.is_empty() {
                        Vec::new()
                    } else {
                        vec![(Object(K::Environment), environments)]
                    },
                ));
            }
            primary.extend(facts.clone());
            let bound = native::bind_known(ctx, input.mode.row(), &primary)?;
            let reader =
                native::preparation_principal(ctx, conn, principal.caller(), &bound).await?;
            let root = content_sources::prepare(conn, &reader, input.id).await?;
            let child = if let Some(id) = input.thread {
                Some(content_sources::prepare(conn, &reader, id).await?)
            } else {
                None
            };
            Ok((Value { names, root, child }, facts))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Rendered>> {
        Box::pin(async move {
            let node = if !native::modern(principal.caller()) {
                shown(ctx, conn, principal, input.id).await?
            } else {
                view::one_with_name_sources(
                    conn,
                    principal,
                    input.id,
                    ctx.now,
                    Policy::default(),
                    &opened.value.names,
                    opened.value.root.topic.as_ref(),
                )
                .await?
                .ok_or_else(ApiError::not_found)?
            };
            let thread = if let Some(id) = input.thread {
                let node = if !native::modern(principal.caller()) {
                    shown(ctx, conn, principal, id).await?
                } else {
                    view::one_with_name_sources(
                        conn,
                        principal,
                        id,
                        ctx.now,
                        Policy::default(),
                        &opened.value.names,
                        None,
                    )
                    .await?
                    .ok_or_else(ApiError::not_found)?
                };
                match node {
                    Node::Thread(thread) => Some(thread),
                    _ => return Err(ApiError::not_found()),
                }
            } else {
                None
            };
            Ok(Rendered { node, thread })
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.parent == fresh.subject.parent
                && old.subject.child_topic == fresh.subject.child_topic
                && old.subject.environment == fresh.subject.environment
                && old.value.names == fresh.value.names
                && old.value.root.preserves(&fresh.value.root)
                && match (&old.value.child, &fresh.value.child) {
                    (None, None) => true,
                    (Some(old), Some(fresh)) => old.preserves(fresh),
                    _ => false,
                })
        })
    }
}
