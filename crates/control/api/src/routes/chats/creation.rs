//! Native chat opening operands and its own completed conversation projection.

use super::*;
use crate::key_operations::native::{self, effects, reply};
use crate::key_operations::targets;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::chat_sources::{self as declarations, Source};
use orch_api_contract::chats::{ChatCreateReceipt, ChatCreateResponse};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Caller;
use orch_authz::engine::Graph;

const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);
const GROUP: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Group);
const NODES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::NameOrgs),
    (ObjectKind::Project, Source::NameProjects),
    (ObjectKind::Topic, Source::NameTopics),
    (ObjectKind::Thread, Source::NameThreads),
];

fn work(ctx: &Ctx, principal: &Principal) -> Work {
    Work {
        projection_ctx: ctx.clone(),
        principal: principal.clone(),
        origin: origin(ctx),
        now: principal.now().get(),
    }
}

async fn parent(conn: &mut Conn, root: Id) -> ApiResult<native::Facts<Source>> {
    let target = targets::node(conn, root).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Org) {
        return Err(ApiError::not_found());
    }
    Ok(native::Facts::one(Source::Parent, CHAT, target))
}

pub(super) struct Create;
impl effects::Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = OrgChatCreateBody;
    type Value = chats::PreparedOpen;
    type Output = chats::Opened;

    fn declaration(_value: &Self::Value) -> &'static Declaration<Source> {
        &declarations::CREATE
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: OrgChatCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let root = ctx.id("id")?;
            let facts = vec![parent(conn, root).await?];
            let base = native::bind_known(ctx, &declarations::CREATE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &base).await?;
            let acting = work(ctx, &principal);
            let result = chats::prepare_open(
                conn,
                &acting.acting(),
                root,
                &body.user_ids,
                body.name.as_deref(),
            )
            .await;
            let prepared = acting.refuse(conn, None, result).await?;
            let actual = vec![parent(conn, prepared.root()).await?];
            Ok((prepared, actual))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let acting = work(ctx, principal);
            let checked = chats::check_open_preparation(conn, &acting.acting(), value).await;
            acting.refuse(conn, None, checked).await
        })
    }
}

impl effects::Apply for Create {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            let acting = work(ctx, principal);
            // apply_open has no awaited validation: its first await attempts the native INSERT.
            before.check()?;
            let result = chats::apply_open(conn, &acting.acting(), value).await;
            Ok(effects::Outcome::Applied(
                acting.refuse(conn, None, result).await?,
            ))
        })
    }
}

pub(super) async fn response(
    completed: effects::Completed<chats::Opened>,
) -> ApiResult<Answer<ChatCreateResponse>> {
    let created = completed.output.created;
    let receipt = ChatCreateResponse::Receipt(ChatCreateReceipt {
        projection: ReceiptProjection::Receipt,
        chat_id: completed.output.chat.id,
        created,
    });
    crate::key_operations::publication::receipt(&completed.ctx, &receipt)?;
    let value =
        match reply::render::<Readback>(&completed.ctx, &completed.accepted, completed.output)
            .await?
        {
            Some(chat) => ChatCreateResponse::Full(Box::new(chat)),
            None => receipt,
        };
    Ok(if created {
        Answer::from(value)
    } else {
        Answer::from(value).status(StatusCode::OK)
    })
}

struct Readback;
impl reply::Projection for Readback {
    type Source = Source;
    type Input = chats::Opened;
    type Subject = ChatRow;
    type Value = read_operation::PreparedRead;
    type Reply = Chat;
    const READ_SOURCE: Source = Source::OpenedChat;

    fn declaration(_input: &Self::Input) -> &'static Declaration<Source> {
        &declarations::CREATE
    }

    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::chat(conn, input.chat.id)
                .await?
                .filter(|row| row.org_id == input.chat.org_id && row.kind == input.chat.kind)
                .ok_or_else(ApiError::not_found)?;
            let facts = vec![
                parent(conn, row.org_id).await?,
                native::Facts::one(
                    Source::OpenedChat,
                    CHAT,
                    targets::from_chat(conn, &row).await?,
                ),
            ];
            Ok((row, facts))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Self::Input,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            load(conn, &work(ctx, principal), subject.id, Mode::Read)
                .await
                .map(|_| ())
        })
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Self::Input,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let loaded = load(conn, &work(ctx, principal), subject.id, Mode::Read).await?;
            let roots = vec![
                parent(conn, subject.org_id).await?,
                native::Facts::one(
                    Source::OpenedChat,
                    CHAT,
                    targets::from_chat(conn, subject).await?,
                ),
            ];
            let known = native::bind_required(ctx, &declarations::CREATE, &roots)?;
            let (plan, prepared) =
                read_operation::prepare_completed_projection(conn, principal, known, loaded)
                    .await?;
            let graph = Graph::load(
                conn,
                &prepared.name_nodes().iter().copied().collect::<Vec<_>>(),
            )
            .await
            .map_err(ApiError::internal)?;
            let (mut facts, _) = native::node_name_facts(
                &plan,
                &declarations::CREATE,
                principal.caller(),
                &graph,
                prepared.name_nodes(),
                NODES,
            )?;
            let inspections = crate::routes::access::operations::group_targets(
                conn,
                prepared.inspection_groups(),
            )
            .await?;
            facts.push(native::Facts::collection(
                Source::InspectionGroups,
                if inspections.is_empty() {
                    Vec::new()
                } else {
                    vec![(GROUP, inspections)]
                },
            ));
            let groups = prepared.message_groups().to_vec();
            facts.push(native::Facts::collection(
                Source::NameGroups,
                if groups.is_empty() {
                    Vec::new()
                } else {
                    vec![(GROUP, groups)]
                },
            ));
            Ok((prepared, facts))
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Self::Input,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Chat>> {
        Box::pin(async move {
            read_operation::project(conn, principal, &prepared.value)
                .await?
                .chat
                .ok_or_else(ApiError::not_found)
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Self::Input,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject.id == fresh.subject.id
                && opened.subject.org_id == fresh.subject.org_id
                && opened.subject.kind == fresh.subject.kind
                && opened.value.loaded.standing == fresh.value.loaded.standing
                && opened.value.loaded.policy == fresh.value.loaded.policy)
        })
    }
}
