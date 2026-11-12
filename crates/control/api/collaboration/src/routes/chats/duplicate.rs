//! A source-qualified group copy uses the native opening, naming, retained bytes and forwarding writers.

use super::*;

use crate::key_operations::blob_sources;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::targets;

use crate::routes::duplicates;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::duplicate_sources as sources;

use orch_api_contract::duplicate_sources::Source;

use orch_api_contract::duplicates::ChatDuplicate;

use orch_api_contract::duplicates::ChatDuplicateBody;

use orch_api_contract::duplicates::ChatMoveOut;

use orch_api_contract::duplicates::ChatMoveOutBody;

use orch_api_contract::duplicates::ChatMoveOutPlan;

use orch_api_contract::duplicates::ChatMoveOutPreview;

use orch_api_contract::duplicates::CopySubject;

use orch_api_contract::duplicates::CopySubjectKind;

use orch_api_contract::duplicates::DuplicateBody;

use orch_api_contract::duplicates::DuplicateDestination;

use orch_api_contract::duplicates::DuplicateKind;

use orch_api_contract::duplicates::MoveOutDeleteEffect;

use orch_api_contract::duplicates::MoveOutDeletePlan;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_chat::following::BlobOrigin;

use orch_types::Hash;

use std::collections::BTreeSet;
const CHAT: KeyTargetKind = KeyTargetKind::Identity(KeyIdentityKind::Chat);

const PAGE: u32 = orch_api_contract::chats::FORWARD_MAX_MESSAGES;

const NAMES: &[(KeyTargetKind, Source)] = &[
    (
        KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Org),
        Source::NameOrgs,
    ),
    (
        KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Project),
        Source::NameProjects,
    ),
    (
        KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Topic),
        Source::NameTopics,
    ),
    (
        KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Thread),
        Source::NameThreads,
    ),
    (
        KeyTargetKind::Identity(KeyIdentityKind::Group),
        Source::NameGroups,
    ),
];

pub(crate) struct Groups;

struct Moving;

impl duplicates::Configuration for Moving {
    const KIND: DuplicateKind = DuplicateKind::Chat;
    const REPLAY: &'static str = "chat_move_out";
    const MOVING: bool = true;
}

impl duplicates::Configuration for Groups {
    const KIND: DuplicateKind = DuplicateKind::Chat;
    const REPLAY: &'static str = "chat_duplicate";
}

pub(crate) struct Prepared {
    source: Id,
    copy: DuplicateBody,
    source_root: Id,
    root: Id,
    home: Id,
    open: chats::PreparedOpen,
    names: names::AllowSet,
    blobs: Vec<(Hash, BlobOrigin)>,
    notices: Vec<Id>,
    delete: Option<MoveOutDeletePlan>,
    audience: Option<crate::routes::notifications::person::CopyAudience>,
    factual: Vec<native::Facts<Source>>,
}

pub(crate) fn mount(registry: &mut Registry) {
    duplicates::register::<Groups, _>(registry, ChatDuplicate);
    duplicates::register::<Moving, _>(registry, ChatMoveOut);
    moving::mount(registry);
    preview::mount(registry);
}
orch_api_base::__api_bind_fce0a68441!();

/// The writer and read-only destination picker ask the same native group-opening gate.
pub(crate) async fn copy_destination(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    source: Id,
    root: Id,
    home: Id,
    name: &str,
) -> ApiResult<chats::PreparedOpen> {
    let work = creation::work(ctx, principal);
    let open = chats::prepare_copy(conn, &work.acting(), source, root, home, name).await;
    work.refuse(conn, Some(source), open).await
}
orch_api_base::__api_bind_b088dc274d!();

async fn facts(conn: &mut Conn, source: Id, home: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![
        native::Facts::one(Source::Subject, CHAT, targets::chat(conn, source).await?),
        native::Facts::one(Source::Destination, CHAT, targets::node(conn, home).await?),
    ])
}

/// Read every native page to factual exhaustion; retain distinct source identities, not the whole transcript.
async fn census(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    act: &Acting<'_>,
    source: Id,
) -> ApiResult<(names::AllowSet, Vec<Hash>)> {
    let mut after = 0;
    let mut names = names::AllowSet::new();
    let mut hashes = BTreeSet::new();
    loop {
        let page = rows::after(conn, source, after, i64::from(PAGE)).await?;
        duplicates::current(ctx, caller)?;
        let Some(last) = page.last() else {
            break;
        };
        after = last.seq_created;
        let selected = page
            .iter()
            .filter(|row| row.kind == rows::MESSAGE && !row.is_deleted())
            .collect::<Vec<_>>();
        if selected.is_empty() {
            continue;
        }
        let ids = selected.iter().map(|row| row.id).collect::<Vec<_>>();
        let prepared = forward::prepare(conn, act, source, &ids)
            .await
            .map_err(ApiError::internal)?;
        names.extend(
            names::candidate_sources(conn, &selected, &[], Some(&prepared.chat.row)).await?,
        );
        for row in selected {
            hashes.extend(orch_chat::attachments::blobs_in(&views::parts_of(
                &row.content,
            )));
        }
        duplicates::current(ctx, caller)?;
    }
    Ok((names, hashes.into_iter().collect()))
}

/// Pin the actual metadata identities and ancestry before resolving copied live labels.
struct Footprint {
    nodes: BTreeSet<Id>,
    groups: BTreeSet<Id>,
}

impl Footprint {
    fn covers(&self, target: &KeyResolvedTarget) -> bool {
        let object = match target.object.kind {
            KeyTargetKind::Identity(KeyIdentityKind::Group) => {
                target.object.id.is_some_and(|id| self.groups.contains(&id))
            }
            _ => target.object.id.is_some_and(|id| self.nodes.contains(&id)),
        };
        object
            && target
                .node_ancestors
                .iter()
                .filter_map(|node| node.id)
                .all(|id| self.nodes.contains(&id))
    }
}

async fn lock_names(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    candidates: &names::AllowSet,
) -> ApiResult<Footprint> {
    let groups = candidates
        .iter()
        .filter_map(|source| match source {
            names::NameSource::Group(id) => Some(*id),
            _ => None,
        })
        .collect::<Vec<_>>();
    let held_groups: Vec<Id> =
        sqlx::query_scalar("SELECT id FROM groups WHERE id=ANY($1) ORDER BY id FOR SHARE")
            .bind(&groups)
            .fetch_all(&mut *conn)
            .await
            .map_err(DbError::from)?;
    duplicates::current(ctx, caller)?;
    let group_targets = crate::routes::access::operations::group_targets(conn, &groups).await?;
    let seeds = candidates
        .iter()
        .filter_map(|source| match source {
            names::NameSource::Node(id) => Some(*id),
            _ => None,
        })
        .chain(
            group_targets
                .iter()
                .flat_map(|target| target.node_ancestors.iter().filter_map(|node| node.id)),
        )
        .collect::<Vec<_>>();
    let mut locked = BTreeSet::new();
    loop {
        let graph = orch_authz::engine::Graph::load(conn, &seeds)
            .await
            .map_err(ApiError::internal)?;
        let needed = seeds
            .iter()
            .filter_map(|id| graph.get(*id))
            .flat_map(|node| node.path.iter().copied())
            .collect::<BTreeSet<_>>();
        let more = needed.difference(&locked).copied().collect::<Vec<_>>();
        if more.is_empty() {
            break;
        }
        let held: Vec<Id> =
            sqlx::query_scalar("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
                .bind(&more)
                .fetch_all(&mut *conn)
                .await
                .map_err(DbError::from)?;
        if held != more {
            return Err(DbError::Retry.into());
        }
        locked.extend(held);
        duplicates::current(ctx, caller)?;
    }
    Ok(Footprint {
        nodes: locked,
        groups: held_groups.into_iter().collect(),
    })
}

fn name_facts(allowed: &[(names::NameSource, KeyResolvedTarget)]) -> Vec<native::Facts<Source>> {
    NAMES
        .iter()
        .map(|(kind, source)| {
            let selected = allowed
                .iter()
                .filter(|(_, target)| target.object.kind == *kind)
                .map(|(_, target)| target.clone())
                .collect::<Vec<_>>();
            native::Facts::collection(
                *source,
                if selected.is_empty() {
                    Vec::new()
                } else {
                    vec![(*kind, selected)]
                },
            )
        })
        .collect()
}

fn blob_facts(bindings: &[blob_sources::Binding]) -> Vec<native::Facts<Source>> {
    use blob_sources::SourceKind;
    [
        (SourceKind::Chat, Source::BlobChats),
        (SourceKind::Thread, Source::BlobThreads),
        (SourceKind::Document, Source::BlobDocuments),
        (SourceKind::Environment, Source::BlobEnvironments),
        (SourceKind::RetainedUpload, Source::BlobUploads),
    ]
    .into_iter()
    .map(|(kind, source)| {
        let targets = bindings
            .iter()
            .filter(|binding| binding.source == kind)
            .map(|binding| binding.target.clone())
            .collect::<Vec<_>>();
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

enum Intent {
    Copy(ChatDuplicateBody),
    Preview(DuplicateBody),
    Move(ChatMoveOutBody),
}

impl Intent {
    fn declaration(&self) -> &'static Declaration<Source> {
        match self {
            Self::Copy(_) => &sources::CHAT,
            Self::Preview(_) => &sources::CHAT_MOVE_PREVIEW,
            Self::Move(_) => &sources::CHAT_MOVE,
        }
    }
}

async fn prepare_native(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    intent: Intent,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let preview = matches!(&intent, Intent::Preview(_));
    let moving = !matches!(&intent, Intent::Copy(_));
    let declaration = intent.declaration();
    let (body, confirmation) = match intent {
        Intent::Copy(body) => (body, None),
        Intent::Preview(copy) => (
            ChatDuplicateBody {
                copy,
                ..Default::default()
            },
            None,
        ),
        Intent::Move(body) => (
            ChatDuplicateBody {
                copy: body.copy,
                notify: body.notify,
                notify_preview_id: body.notify_preview_id,
            },
            Some(body.delete_plan),
        ),
    };
    let options = &body.copy;
    let source = rows::lock(conn, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let home = duplicates::destination(options, Some(source.home())).ok_or_else(|| {
        ApiError::invalid("destination", "a group chat is at home in an organization")
    })?;
    let mut factual = facts(conn, source.id, home).await?;
    let base = native::bind_known(ctx, declaration, &factual)?;
    let principal = base.preparation_principal(ctx, conn, caller).await?;
    let work = creation::work(ctx, &principal);
    let loaded = chats::load(conn, &work.acting(), source.id, Mode::Read).await;
    let loaded = work.refuse(conn, Some(source.id), loaded).await?;
    if loaded.row.kind != rows::KIND_GROUP {
        return Err(ApiError::invalid(
            "source",
            "only a group chat is duplicated",
        ));
    }
    duplicates::revision(options, source.seq_state)?;
    let root = duplicates::copy_to(
        conn,
        &principal,
        orch_engine::copy_out::Subject::Chat(source.id),
        Some(home),
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    let delete = if moving {
        if root == source.org_id {
            return Err(ApiError::invalid(
                "destination",
                "Use the existing Move within this organization.",
            ));
        }
        let members = move_owner(ctx, conn, &principal, source.id).await?;
        let plan = move_plan(&source, &members, &principal, options, home)?;
        if let Some(confirmation) = &confirmation {
            duplicates::moving::confirm(&plan, confirmation)?;
        }
        if preview {
            factual.push(native::Facts::one(
                Source::PreviewContent,
                CHAT,
                targets::chat(conn, source.id).await?,
            ));
        }
        Some(plan)
    } else {
        None
    };
    let name = duplicates::display_name(options, source.name.as_deref().unwrap_or("Group chat"));
    let open = copy_destination(ctx, conn, &principal, source.id, root, home, &name).await?;
    let (candidates, hashes) =
        census(ctx, conn, principal.caller(), &work.acting(), source.id).await?;
    let footprint = lock_names(ctx, conn, principal.caller(), &candidates).await?;
    let allowed = operation::allowed_names(conn, caller, &base, candidates).await?;
    if allowed.iter().any(|(_, target)| !footprint.covers(target)) {
        return Err(DbError::Retry.into());
    }
    let selected = allowed
        .iter()
        .map(|(source, _)| *source)
        .collect::<names::AllowSet>();
    let witnesses = hashes
        .iter()
        .map(|hash| (*hash, BlobOrigin::Chat(source.id)))
        .collect::<Vec<_>>();
    let blobs =
        blob_sources::prepare_selected(ctx, conn, caller, &base, &hashes, &[], &witnesses).await?;
    let allowed_facts = name_facts(&allowed);
    factual.extend(allowed_facts);
    factual.extend(blob_facts(&blobs.bindings));
    let audience = if moving {
        Some(
            crate::routes::notifications::person::move_audience(ctx, conn, &principal, source.id)
                .await?,
        )
    } else {
        None
    };
    let notices = if body.notify == Some(true) {
        let audience = if moving {
            crate::routes::notifications::person::move_audience(ctx, conn, &principal, source.id)
                .await?
        } else {
            crate::routes::notifications::person::copy_audience(ctx, conn, &principal, source.id)
                .await?
        };
        if audience.people
            > u64::try_from(orch_api_contract::notifications::PERSON_NOTIFY_MAX_PEOPLE)
                .map_err(ApiError::internal)?
        {
            return Err(ApiError::invalid(
                "notify",
                &format!(
                    "one explicit notice tells at most {} people; duplicate without notices for a larger audience",
                    orch_api_contract::notifications::PERSON_NOTIFY_MAX_PEOPLE
                ),
            ));
        }
        if !audience.users.is_empty() {
            let review = body.notify_preview_id.ok_or_else(|| {
                ApiError::invalid(
                    "notify_preview_id",
                    "review the current contributors before notifying them",
                )
            })?;
            let preview = orch_db::queries::jobs::forks::lock_preflight(conn, review)
                .await?
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::ConflictStaleRevision,
                        "the contributor preview is no longer available",
                    )
                })?;
            if preview.kind
                != if moving {
                    "chat_move_out"
                } else {
                    "chat_duplicate"
                }
                || preview.created_by != principal.user()
                || preview.plan.get("source_chat") != Some(&serde_json::json!(source.id))
                || moving
                    && preview.plan.get("delete_stamp")
                        != delete
                            .as_ref()
                            .map(|plan| serde_json::json!(plan.stamp))
                            .as_ref()
                || preview.expires_at <= ctx.app.clock.now()
            {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "review the current contributors again before notifying them",
                ));
            }
            if !orch_db::queries::notifications::operations::matches_preview(
                conn,
                review,
                &audience.users,
            )
            .await?
            {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the contributors changed since the notice choice was reviewed",
                ));
            }
        }
        audience.users
    } else {
        Vec::new()
    };
    duplicates::current(ctx, principal.caller())?;
    Ok((
        Prepared {
            source: source.id,
            copy: options.clone(),
            source_root: source.org_id,
            root,
            home,
            open,
            names: selected,
            blobs: blobs.selected,
            notices,
            delete,
            audience,
            factual: factual.clone(),
        },
        factual,
    ))
}

async fn require_native(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Prepared,
) -> ApiResult<()> {
    let work = creation::work(ctx, principal);
    let loaded = chats::load(conn, &work.acting(), value.source, Mode::Read).await;
    let loaded = work.refuse(conn, Some(value.source), loaded).await?;
    if loaded.row.kind != rows::KIND_GROUP || loaded.row.org_id != value.source_root {
        return Err(ApiError::not_found());
    }
    if value.delete.is_some() {
        move_owner(ctx, conn, principal, value.source).await?;
    }
    duplicates::copy_to(
        conn,
        principal,
        orch_engine::copy_out::Subject::Chat(value.source),
        Some(value.home),
    )
    .await?;
    let checked = chats::check_open_preparation(conn, &work.acting(), &value.open).await;
    work.refuse(conn, Some(value.source), checked).await
}

async fn apply_native<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    principal: &'a Principal,
    value: &'a Prepared,
    before: effects::FirstEffect<'a>,
) -> ApiResult<effects::Outcome<Id>> {
    let work = creation::work(ctx, principal);
    let deadline = before.deadline();
    before.check()?;
    let source = chats::load(conn, &work.acting(), value.source, Mode::Read).await;
    let source = work.refuse(conn, Some(value.source), source).await?;
    deadline.check()?;
    let audited = inspection::note_opened(conn, &work.acting(), &source).await;
    work.refuse(conn, Some(value.source), audited).await?;
    deadline.check()?;
    let opened = chats::apply_open(conn, &work.acting(), &value.open).await;
    let opened = work.refuse(conn, Some(value.source), opened).await?;
    deadline.check()?;
    if !opened.created || opened.chat.kind != rows::KIND_GROUP {
        return Err(ApiError::internal(
            "the native chat copy did not create a new group",
        ));
    }
    let target = opened.chat.id;
    let mut after = 0;
    loop {
        let page = rows::after(conn, value.source, after, i64::from(PAGE)).await?;
        deadline.check()?;
        let Some(last) = page.last() else {
            break;
        };
        after = last.seq_created;
        let ids = page
            .iter()
            .filter(|row| row.kind == rows::MESSAGE && !row.is_deleted())
            .map(|row| row.id)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            continue;
        }
        let prepared = forward::prepare(conn, &work.acting(), value.source, &ids).await;
        let prepared = work.refuse(conn, Some(value.source), prepared).await?;
        deadline.check()?;
        for row in &prepared.messages {
            let capture = names::capture(
                conn,
                principal,
                &[row],
                &[],
                Some(&prepared.chat.row),
                Some(&value.names),
            )
            .await?;
            let reference =
                names::chat_reference(conn, principal, &prepared.chat.row, Some(&value.names))
                    .await?;
            let one = forward::Prepared {
                chat: prepared.chat.clone(),
                messages: vec![row.clone()],
            };
            let copies = forward::copies_of(&one, &reference.reference, &capture.lookups);
            let maximum = usize::try_from(orch_api_contract::chats::CHAT_MESSAGE_MAX_PARTS)
                .map_err(ApiError::internal)?;
            for (index, parts) in copies.chunks(maximum).enumerate() {
                let destinations = [Destination::Chat(target)];
                let plans = [forward::PlannedDestination {
                    requested: destinations[0],
                    state: forward::DestinationState::ExistingChat(target),
                }];
                let client = format!("copy:{target}:{}:{index}", row.id);
                let result = forward::deliver_checked(
                    conn,
                    &work.acting(),
                    &Delivery {
                        root: value.root,
                        content: parts.to_vec(),
                        only: None,
                        to: &destinations,
                        comment: None,
                        client_id: &client,
                    },
                    &keys,
                    &plans,
                    &value.blobs,
                )
                .await;
                work.refuse(conn, Some(value.source), result).await?;
                deadline.check()?;
            }
        }
    }
    if !value.notices.is_empty() {
        if value.delete.is_some() {
            let receipt = orch_api_contract::duplicates::DuplicateReceipt {
                projection: orch_api_contract::common::ReceiptProjection::Receipt,
                kind: DuplicateKind::Chat,
                id: target,
                setup: None,
            };
            crate::routes::notifications::person::tell_chat_move(
                ctx,
                conn,
                principal,
                value.source,
                &receipt,
                &value.notices,
            )
            .await?;
        } else {
            crate::routes::notifications::person::tell_chat_copy(
                ctx,
                conn,
                principal,
                value.source,
                target,
                &value.notices,
            )
            .await?;
        }
        deadline.check()?;
    }
    if value.delete.is_some() {
        duplicates::record_copy(ctx, conn, DuplicateKind::Chat, value.source, target).await?;
        deadline.check()?;
        move_owner(ctx, conn, principal, value.source).await?;
        deadline.check()?;
        let ended = chats::end_after_move(conn, &work.acting(), &value.open, &opened).await;
        work.refuse(conn, Some(value.source), ended).await?;
        deadline.check()?;
    }
    Ok(effects::Outcome::Applied(target))
}

macro_rules! group_effect {
    ($family:ty,$body:ty,$row:expr,$intent:ident) => {
        impl effects::Effect for $family {
            type Source = Source;
            type Query = ();
            type Body = $body;
            type Value = Prepared;
            type Output = Id;
            fn declaration(_: &Prepared) -> &'static Declaration<Source> {
                $row
            }
            fn prepare<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                caller: &'a Caller,
                (): (),
                body: $body,
            ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
                Box::pin(prepare_native(ctx, conn, caller, Intent::$intent(body)))
            }
            fn require<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Prepared,
            ) -> BoxFuture<'a, ApiResult<()>> {
                Box::pin(require_native(ctx, conn, principal, value))
            }
        }
        impl effects::Apply for $family {
            fn apply<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Prepared,
                before: effects::FirstEffect<'a>,
            ) -> BoxFuture<'a, ApiResult<effects::Outcome<Id>>> {
                Box::pin(apply_native(ctx, conn, principal, value, before))
            }
        }
    };
}

group_effect!(Groups, ChatDuplicateBody, &sources::CHAT, Copy);

group_effect!(Moving, ChatMoveOutBody, &sources::CHAT_MOVE, Move);

async fn move_owner(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    source: Id,
) -> ApiResult<Vec<rows::MemberRow>> {
    if !orch_engine::copy_out::owns(
        conn,
        principal,
        orch_engine::copy_out::Subject::Chat(source),
    )
    .await
    .map_err(duplicates::copy_error)?
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "Only an active manager can move this group into another org.",
        ));
    }
    let members = rows::members(conn, source).await?;
    let own = members
        .iter()
        .find(|member| {
            member.user_id == principal.user()
                && member.manager
                && member.active_at(ctx.app.clock.now())
        })
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "The group's current manager standing ended.",
            )
        })?;
    ctx.publication
        .retain_deadline(own.expires_at.map(orch_types::Time::new))?;
    Ok(members)
}

fn move_plan(
    row: &ChatRow,
    members: &[rows::MemberRow],
    principal: &Principal,
    copy: &DuplicateBody,
    home: Id,
) -> ApiResult<MoveOutDeletePlan> {
    let mut roster:Vec<_>=members.iter().map(|member|(member.user_id,serde_json::json!({"user":member.user_id,"manager":member.manager,"guest":member.guest,"left":member.left_at,"expires":member.expires_at,"revision":member.seq_updated}))).collect();
    roster.sort_by_key(|row| row.0);
    duplicates::moving::plan(
        CopySubject {
            kind: CopySubjectKind::Chat,
            id: row.id,
        },
        DuplicateDestination::Node { id: home },
        row.seq_state,
        principal.user(),
        copy,
        serde_json::json!({"root":row.org_id,"home":row.home(),"sequence":row.next_seq,"name":row.name,"roster":roster}),
        vec![MoveOutDeleteEffect::EndGroupAndHistory],
    )
}

#[path = "duplicate/moving.rs"]
pub mod moving;

#[path = "duplicate/preview.rs"]
pub mod preview;
