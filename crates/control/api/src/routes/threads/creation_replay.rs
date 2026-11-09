//! Current native ThreadCreate response, with the original creator's exact metadata cuts.
use super::*;
use crate::key_operations::native::{self, reply};
use crate::key_operations::targets;
use crate::routes::nodes::content_sources;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind::Object;
use orch_access_contract::permissions::Permission;
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind as K};
use orch_api_contract::{
    common::ReceiptProjection,
    nodes::{CreateThread, ThreadCreateReceipt, ThreadCreateResponse},
    operation_sources::{Binding, Cardinality, Declaration},
    operations::{FacetUsage, TargetResolver as Target},
};
use orch_authz::{Act, Principal};
use orch_types::Json;
use std::collections::BTreeSet;

const ROW: Declaration<&'static str> = Declaration {
    operation: "ThreadCreate",
    bindings: &[
        Binding {
            source: "topic",
            target: Target::PermissionObject,
            usage: FacetUsage::Required,
            cardinality: Cardinality::One,
        },
        Binding {
            source: "created_resource",
            target: Target::Resolved("created_resource"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::One,
        },
        Binding {
            source: "creator_new_environment_homes",
            target: Target::ResolvedCollection("creator_new_environment_homes"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_initial_file_threads",
            target: Target::ResolvedCollection("creator_initial_file_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_initial_file_environments",
            target: Target::ResolvedCollection("creator_initial_file_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_file_environments",
            target: Target::ResolvedCollection("creator_file_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_reader_environments",
            target: Target::ResolvedCollection("creator_reader_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_file_sources",
            target: Target::ResolvedCollection("creator_file_sources"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_agent_projects",
            target: Target::ResolvedCollection("creator_agent_projects"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_agent_topics",
            target: Target::ResolvedCollection("creator_agent_topics"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_agent_threads",
            target: Target::ResolvedCollection("creator_agent_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "message_source_chats",
            target: Target::ResolvedCollection("message_source_chats"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "message_source_threads",
            target: Target::ResolvedCollection("message_source_threads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "message_source_documents",
            target: Target::ResolvedCollection("message_source_documents"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "message_source_environments",
            target: Target::ResolvedCollection("message_source_environments"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "message_retained_uploads",
            target: Target::ResolvedCollection("message_retained_uploads"),
            usage: FacetUsage::Required,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_names_orgs",
            target: Target::ResolvedCollection("creator_names_orgs"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_names_projects",
            target: Target::ResolvedCollection("creator_names_projects"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_names_topics",
            target: Target::ResolvedCollection("creator_names_topics"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
        Binding {
            source: "creator_environments",
            target: Target::ResolvedCollection("creator_environments"),
            usage: FacetUsage::ResponseContent,
            cardinality: Cardinality::Collection,
        },
    ],
};
pub(super) fn binding()
-> crate::middleware::idempotency::Creation<(), CreateThread, ThreadCreateResponse> {
    crate::middleware::idempotency::Creation {
        kind: "thread",
        replay,
        identity,
    }
}
fn identity(value: &ThreadCreateResponse) -> ApiResult<orch_db::idempotency::creation::Created> {
    let (id, topic, input) = match value {
        ThreadCreateResponse::Full(value) => (value.base.id, value.topic_id, None),
        ThreadCreateResponse::Receipt(value) => (value.thread_id, value.topic_id, value.input_id),
    };
    Ok(orch_db::idempotency::creation::Created {
        kind: "thread".into(),
        id,
        data: serde_json::json!({"topic":topic,"input":input}),
    })
}
fn replay(
    ctx: Ctx,
    (): (),
    body: CreateThread,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<ThreadCreateResponse>> {
    Box::pin(async move {
        let topic = created
            .data
            .get("topic")
            .and_then(Json::as_str)
            .and_then(|id| id.parse().ok())
            .unwrap_or(ctx.id("id")?);
        let mut receipt = ThreadCreateReceipt {
            projection: ReceiptProjection::Receipt,
            thread_id: created.id,
            topic_id: topic,
            input_id: created
                .data
                .get("input")
                .cloned()
                .map(serde_json::from_value::<Option<Id>>)
                .transpose()
                .map_err(ApiError::internal)?
                .flatten(),
        };
        if receipt.input_id.is_none() {
            let mut conn = ctx.app.db.acquire().await?;
            receipt.input_id = super::creation_operation::receipt(
                &mut conn,
                ctx.user()?.id(),
                created.id,
                topic,
                body.first_input.as_ref(),
            )
            .await?
            .input_id;
        }
        let receipt = ThreadCreateResponse::Receipt(receipt);
        crate::key_operations::publication::receipt(&ctx, &receipt)?;
        match reply::render_completed_replay::<Current>(&ctx, ctx.user()?.id(), created.id).await {
            Ok(Some(thread)) => Ok(ThreadCreateResponse::full(thread)),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}
struct Current;
struct Subject {
    id: Id,
    topic: Id,
    environment: Option<Id>,
}
struct View {
    names: BTreeSet<Id>,
    witness: content_sources::Prepared,
}
impl reply::Projection for Current {
    type Source = &'static str;
    type Input = Id;
    type Subject = Subject;
    type Value = View;
    type Reply = Thread;
    const READ_SOURCE: &'static str = "created_resource";
    fn declaration(_: &Id) -> &'static Declaration<Self::Source> {
        &ROW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        id: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Self::Source>>)>> {
        Box::pin(async move {
            let row = orch_db::queries::threads::thread(conn, *id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let target = targets::node(conn, *id).await?;
            Ok((
                Subject {
                    id: *id,
                    topic: row.topic_id,
                    environment: row.environment_id,
                },
                vec![native::Facts::one(
                    "created_resource",
                    Object(K::Thread),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Id,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require(
                conn,
                principal,
                subject.id,
                authorization::exact(Permission::ThreadView),
                Act::Use,
                Policy::default(),
            )
            .await
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Id,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(View, Vec<native::Facts<Self::Source>>)>> {
        Box::pin(async move {
            let root = targets::node(conn, subject.id).await?;
            let known = native::bind_known(
                ctx,
                &ROW,
                &[native::Facts::one(
                    "created_resource",
                    Object(K::Thread),
                    root.clone(),
                )],
            )?;
            let mut names = BTreeSet::from([subject.id]);
            let mut facts = Vec::new();
            for (role, kind) in [
                ("creator_names_orgs", K::Org),
                ("creator_names_projects", K::Project),
                ("creator_names_topics", K::Topic),
            ] {
                let mut selected = Vec::new();
                for ancestor in &root.node_ancestors {
                    if ancestor.kind != Object(kind) {
                        continue;
                    }
                    let target =
                        targets::node(conn, ancestor.id.ok_or_else(ApiError::not_found)?).await?;
                    if !native::modern(principal.caller())
                        || native::permits_usage(
                            &known,
                            &ROW,
                            principal.caller(),
                            &role,
                            FacetUsage::ResponseContent,
                            Object(kind),
                            &target,
                        )?
                    {
                        names.insert(ancestor.id.ok_or_else(ApiError::not_found)?);
                        selected.push(target);
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
            let environments = match subject.environment {
                Some(id) => vec![(
                    Object(K::Environment),
                    vec![
                        targets::page_object(
                            conn,
                            ObjectRef {
                                kind: K::Environment,
                                id,
                            },
                        )
                        .await?,
                    ],
                )],
                None => Vec::new(),
            };
            facts.push(native::Facts::collection(
                "creator_environments",
                environments,
            ));
            let mut complete = facts.clone();
            complete.push(native::Facts::one(
                "created_resource",
                Object(K::Thread),
                root,
            ));
            let known = native::bind_known(ctx, &ROW, &complete)?;
            let reader =
                native::preparation_principal(ctx, conn, principal.caller(), &known).await?;
            let witness = content_sources::prepare(conn, &reader, subject.id).await?;
            Ok((View { names, witness }, facts))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Id,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Thread>> {
        Box::pin(async move {
            if !native::modern(principal.caller()) {
                return super::one(ctx, conn, prepared.subject.id).await;
            }
            match crate::routes::nodes::view::one_with_name_sources(
                conn,
                principal,
                prepared.subject.id,
                ctx.now,
                Policy::default(),
                &prepared.value.names,
                None,
            )
            .await?
            {
                Some(Node::Thread(thread)) => Ok(thread),
                _ => Err(ApiError::not_found()),
            }
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _: &'a Id,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.topic == fresh.subject.topic
                && old.subject.environment == fresh.subject.environment
                && old.value.names == fresh.value.names
                && old.value.witness.preserves(&fresh.value.witness))
        })
    }
}
