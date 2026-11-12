//! Current own-invitation metadata is read without recovering its first token or issuer credential.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::middleware::idempotency::ObservedCreation;

use futures::future::BoxFuture;

use orch_api_contract::operation_sources::Binding;

use orch_api_contract::operation_sources::Cardinality;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::TargetResolver;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Destination,
    Payload,
    Chats,
    Orgs,
    Projects,
    Topics,
    Threads,
}

const fn one(source: Source, target: &'static str, usage: FacetUsage) -> Binding<Source> {
    Binding {
        source,
        target: TargetResolver::Resolved(target),
        usage,
        cardinality: Cardinality::One,
    }
}

const fn names(source: Source, target: &'static str) -> Binding<Source> {
    Binding {
        source,
        target: TargetResolver::ResolvedCollection(target),
        usage: FacetUsage::ResponseContent,
        cardinality: Cardinality::Collection,
    }
}

const CREATE: Declaration<Source> = Declaration {
    operation: "InvitationCreate",
    bindings: &[
        one(
            Source::Destination,
            "invitation_destination",
            FacetUsage::Required,
        ),
        one(
            Source::Payload,
            "invitation_payload",
            FacetUsage::ResponseContent,
        ),
        names(Source::Chats, "invitation_chats"),
        names(Source::Orgs, "invitation_names_orgs"),
        names(Source::Projects, "invitation_names_projects"),
        names(Source::Topics, "invitation_names_topics"),
        names(Source::Threads, "invitation_names_threads"),
    ],
};

struct Input {
    id: Id,
}

struct Subject {
    row: InvitationRow,
    governing: KeyResolvedTarget,
}

struct Content;

pub(crate) fn binding() -> ObservedCreation<(), InvitationCreateBody, InvitationCreateResponse> {
    ObservedCreation {
        kind: "invitation",
        replay: |ctx, (), _body, created| {
            Box::pin(async move {
                let sent: bool =
                    serde_json::from_value(created.data.get("sent").cloned().ok_or_else(|| {
                        ApiError::internal("the accepted invitation has no mail outcome")
                    })?)
                    .map_err(ApiError::internal)?;
                let receipt = InvitationCreateResponse::Receipt(InvitationCreateReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    invitation_id: created.id,
                    token: None,
                    url: None,
                    sent,
                    already_created: true,
                });
                publication::receipt(&ctx, &receipt)?;
                match reply::render_completed_replay::<Content>(
                    &ctx,
                    ctx.user()?.id(),
                    Input { id: created.id },
                )
                .await
                {
                    Ok(Some(invitation)) => {
                        Ok(InvitationCreateResponse::full(InvitationCreateReply {
                            invitation,
                            token: None,
                            url: None,
                            sent,
                            already_created: true,
                        }))
                    }
                    Ok(None) => Ok(receipt),
                    Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                        Ok(receipt)
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}

impl reply::Projection for Content {
    type Source = Source;
    type Input = Input;
    type Subject = Subject;
    type Value = Invitation;
    type Reply = Invitation;
    const READ_SOURCE: Source = Source::Payload;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &CREATE
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let account = ctx.user()?.id();
            let row = identity::invitation_by_id(conn, input.id)
                .await?
                .filter(|row| row.created_by == account)
                .ok_or_else(ApiError::not_found)?;
            let governing = governing_row(conn, &row).await?;
            let facts = vec![
                native::Facts::one(Source::Destination, INVITATION, governing.clone()),
                native::Facts::one(Source::Payload, INVITATION, governing.clone()),
            ];
            Ok((Subject { row, governing }, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        p: &'a Principal,
        _: &'a Input,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if subject.row.created_by != p.user() {
                return Err(ApiError::not_found());
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        _: &'a Input,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(Invitation, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mut values = invitations_shown(conn, p, std::slice::from_ref(&subject.row)).await?;
            let plan = base(ctx, &subject.governing)?;
            bound_invitation_refs(conn, p.caller(), &plan, &mut values).await?;
            let value = values.into_iter().next().ok_or_else(ApiError::not_found)?;
            let sources = Sources::invitation(&value);
            let mut facts = Vec::new();
            for (kind, source) in [
                (ObjectKind::Org, Source::Orgs),
                (ObjectKind::Project, Source::Projects),
                (ObjectKind::Topic, Source::Topics),
                (ObjectKind::Thread, Source::Threads),
            ] {
                let mut targets = Vec::new();
                for &(_, id) in sources.objects.iter().filter(|(actual, _)| *actual == kind) {
                    targets.push(targets::node(conn, id).await?);
                }
                facts.push(native::Facts::collection(
                    source,
                    if targets.is_empty() {
                        Vec::new()
                    } else {
                        vec![(KeyTargetKind::Object(kind), targets)]
                    },
                ));
            }
            let mut chats = Vec::new();
            for id in sources.chats {
                chats.push(targets::chat_identity(conn, id).await?);
            }
            facts.push(native::Facts::collection(
                Source::Chats,
                if chats.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Identity(KeyIdentityKind::Chat), chats)]
                },
            ));
            Ok((value, facts))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Invitation>> {
        Box::pin(async move { Ok(opened.value.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let a = &old.subject.row;
            let b = &fresh.subject.row;
            Ok(a.id == b.id
                && a.created_by == b.created_by
                && a.kind == b.kind
                && a.org_id == b.org_id
                && a.node_id == b.node_id
                && a.chat_id == b.chat_id
                && old.subject.governing.object == fresh.subject.governing.object)
        })
    }
}
