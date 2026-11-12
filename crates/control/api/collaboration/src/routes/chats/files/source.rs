//! Effect-free native file metadata; byte suppliers run after the producing snapshot.

use super::Kept;

use super::described_in;

use crate::key_operations::blob_sources;

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::chat_sources as declarations;

use orch_api_contract::chat_sources::Source;

use orch_api_contract::chats::ChatBlobGetQuery;

use orch_api_contract::chats::ChatPart;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_chat::chats::Loaded;

use orch_chat::chats::Mode;

use orch_chat::following::BlobOrigin;

use orch_db::Conn;

use orch_db::queries::chats as rows;

use orch_types::Hash;

use orch_types::Id;

use std::sync::Arc;
const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);

#[derive(Clone, PartialEq, Eq)]
enum Origin {
    Message { id: Id, carried: Option<usize> },
    Draft { bytes: Vec<(Hash, BlobOrigin)> },
}

/// Only the exact successful snapshot's native association is retained for later reads.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct Witness {
    chat: Id,
    hash: Hash,
    origin: Origin,
}

pub(super) struct Selection {
    pub(super) loaded: Loaded,
    pub(super) principal: Principal,
    pub(super) kept: Kept,
    pub(super) witness: Witness,
    pub(super) hash: Hash,
}

impl Selection {
    pub(super) fn reference(&self) -> Option<(Id, Id, i32)> {
        let Origin::Message {
            id,
            carried: Some(carried),
        } = &self.witness.origin
        else {
            return None;
        };
        let slot = u64::try_from(*carried).ok()?.to_be_bytes();
        Some((
            orch_engine::ids::derived(
                "shared.chat.file",
                &[self.loaded.row.id.as_bytes(), id.as_bytes(), &slot],
            ),
            *id,
            i32::try_from(*carried).ok()?,
        ))
    }
}

pub(super) struct Blobs;

pub(super) fn work(ctx: &Ctx, principal: &Principal) -> super::super::Work {
    super::super::Work {
        projection_ctx: ctx.clone(),
        principal: principal.clone(),
        now: principal.now().get(),
        origin: crate::sign_in::origin(ctx),
    }
}

fn described(content: orch_types::Json, hash: &Hash) -> Option<(Kept, Option<usize>)> {
    let parts: Vec<ChatPart> = serde_json::from_value(content).ok()?;
    let kept = described_in(&parts, hash)?;
    let carried = crate::routes::documents::references::chat_files(parts)
        .iter()
        .position(|file| file.0 == *hash)
        .map(|at| at + 1);
    Some((kept, carried))
}

pub(super) fn byte_facts(bindings: &[blob_sources::Binding]) -> Vec<native::Facts<Source>> {
    use blob_sources::SourceKind;
    [
        (Source::ByteChats, SourceKind::Chat),
        (Source::ByteThreads, SourceKind::Thread),
        (Source::ByteDocuments, SourceKind::Document),
        (Source::ByteEnvironments, SourceKind::Environment),
        (Source::ByteUploads, SourceKind::RetainedUpload),
    ]
    .into_iter()
    .map(|(source, kind)| {
        let targets: Vec<_> = bindings
            .iter()
            .filter(|binding| binding.source == kind)
            .map(|binding| binding.target.clone())
            .collect();
        native::Facts::collection(
            source,
            if targets.is_empty() {
                Vec::new()
            } else {
                vec![(kind.kind(), targets)]
            },
        )
    })
    .collect()
}

impl native::Selected for Blobs {
    type Source = Source;
    type Query = ChatBlobGetQuery;
    type Value = Arc<Selection>;
    type Reply = Arc<Selection>;
    const DECLARATION: &'static Declaration<Source> = &declarations::BLOB_GET;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a ChatBlobGetQuery,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let chat = ctx.id("id")?;
            let hash = ctx
                .param("hash")?
                .parse::<Hash>()
                .map_err(|_| ApiError::not_found())?;
            let target = targets::chat(conn, chat).await?;
            let primary = native::Facts::one(Source::Chat, CHAT, target);
            let base = native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(&primary))?;
            let principal = native::preparation_principal(ctx, conn, caller, &base).await?;
            let acting = work(ctx, &principal);
            let loaded = super::super::load(conn, &acting, chat, Mode::Read).await?;
            let frozen = ctx.optional_retained_source::<Witness>()?;
            if frozen
                .as_ref()
                .is_some_and(|witness| witness.chat != chat || witness.hash != hash)
            {
                return Err(ApiError::internal(
                    "the retained file changed its chat or hash",
                ));
            }
            let message = match frozen.as_deref().map(|witness| &witness.origin) {
                Some(Origin::Draft { .. }) => None,
                Some(Origin::Message { id, .. }) => {
                    sqlx::query_as::<_, (Id, orch_types::Json)>("SELECT id,content FROM chat_messages WHERE id=$1 AND chat_id=$2 AND kind='message' AND deleted_at IS NULL")
                        .bind(id).bind(chat).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?
                }
                None => rows::message_carrying_row(conn, chat, &hash).await?,
            };
            let saved = message.and_then(|(id, content)| {
                described(content, &hash).map(|(kept, carried)| (id, kept, carried))
            });
            let (kept, origin, bindings) = if let Some((id, kept, carried)) = saved {
                (kept, Origin::Message { id, carried }, Vec::new())
            } else {
                if frozen
                    .as_ref()
                    .is_some_and(|witness| matches!(&witness.origin, Origin::Message { .. }))
                    || loaded.ensure_send().is_err()
                    || loaded.standing.inspects()
                {
                    return Err(ApiError::not_found());
                }
                let kept = rows::draft(conn, principal.user(), chat)
                    .await?
                    .and_then(|(content, _, _)| described(content, &hash).map(|(kept, _)| kept))
                    .ok_or_else(ApiError::not_found)?;
                let prepared = match frozen.as_deref().map(|witness| &witness.origin) {
                    Some(Origin::Draft { bytes }) => {
                        blob_sources::prepare_selected(
                            ctx,
                            conn,
                            principal.caller(),
                            &base,
                            &[hash],
                            &[],
                            bytes,
                        )
                        .await?
                    }
                    _ => {
                        blob_sources::prepare(ctx, conn, principal.caller(), &base, &[hash], &[])
                            .await?
                    }
                };
                (
                    kept,
                    Origin::Draft {
                        bytes: prepared.selected,
                    },
                    prepared.bindings,
                )
            };
            let witness = Witness { chat, hash, origin };
            if frozen
                .as_ref()
                .is_some_and(|earlier| earlier.as_ref() != &witness)
            {
                return Err(ApiError::not_found());
            }
            let mut facts = vec![primary];
            facts.extend(byte_facts(&bindings));
            Ok((
                Arc::new(Selection {
                    loaded,
                    principal,
                    kept,
                    witness,
                    hash,
                }),
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if ctx.user()?.id() != principal.user() {
                return Err(ApiError::internal("the file reader changed account"));
            }
            Ok(())
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(Arc::clone(value)) })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _query: &'a ChatBlobGetQuery,
        opened: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(opened.witness == fresh.witness && opened.kept == fresh.kept) })
    }
}
