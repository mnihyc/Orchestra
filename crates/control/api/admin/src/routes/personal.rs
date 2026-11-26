//! Favorites, recents and read/subscription state; what the interface keeps for a person.
//!
//! Everything here acts on the caller and on nobody else. A favorite or a
//! recent is a kind/id the caller may view; one they lost the right to is no
//! longer listed. The interface state is kept under its declared keys and no
//! others, whole, and no value is larger than the limit; the last view of a
//! project is also kept one project at a time, beside the others. Drafts and the
//! chat states belong to the threads and the chats, and the read position
//! of a thread to the threads routes: none of the three is bound here.

use crate::middleware::authorization;

use crate::middleware::authorization::VIEW;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::invalid;

use crate::routes::nodes::placed;

use crate::routes::nodes::view;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::personal::Favorite;

use orch_api_contract::personal::MeFavoriteDelete;

use orch_api_contract::personal::MeFavoriteList;

use orch_api_contract::personal::MeFavoriteSet;

use orch_api_contract::personal::MeFavoriteSetBody;

use orch_api_contract::personal::MeItemFavoriteDelete;

use orch_api_contract::personal::MeItemFavoriteSet;

use orch_api_contract::personal::MeItemFavoriteSetBody;

use orch_api_contract::personal::{
    MePickerRecentChoice, MePickerRecentCreate, MePickerRecentReceipt, MePickerRecents,
    MePickerRecentsGet,
};

use orch_api_contract::personal::MeProjectViewDelete;

use orch_api_contract::personal::MeProjectViewSet;

use orch_api_contract::personal::MeReadStateList;

use orch_api_contract::personal::MeReadStateListQuery;

use orch_api_contract::personal::MeRecentCreate;

use orch_api_contract::personal::MeRecentCreateBody;

use orch_api_contract::personal::MeStateGet;

use orch_api_contract::personal::MeStateSet;

use orch_api_contract::personal::MeSubscription;

use orch_api_contract::personal::MeSubscriptionList;

use orch_api_contract::personal::MeSubscriptionListQuery;

use orch_api_contract::personal::MeVisitCreate;
use orch_api_contract::personal::{
    MeModelRecentCreate, MeModelRecentList, ModelRecent, ModelRecents,
};

use orch_api_contract::personal::PERSONAL_STATE_MAX_BYTES;

use orch_api_contract::personal::PersonalState;

use orch_api_contract::personal::PersonalStateKey;

use orch_api_contract::personal::PersonalTarget;

use orch_api_contract::personal::ProjectView;

use orch_api_contract::personal::ReadState;

use orch_api_contract::personal::ShortcutState;

use orch_api_contract::personal::SubscriptionSource;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::queries::hierarchy;

use orch_db::queries::hierarchy::Placing;

use orch_db::queries::identity;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(MeRecentCreate, creation_ack::<MeRecentCreateBody>())
        .creation(MeVisitCreate, creation_ack::<PersonalTarget>())
        .creation(MeModelRecentCreate, creation_ack::<ModelRecent>())
        .on_snapshot_read_with_optional_operation(
            MeFavoriteList,
            crate::key_operations::native::selected::<native::Favorites>,
            favorite_list,
        )
        .on(MeFavoriteSet, favorite_set)
        .on(MeFavoriteDelete, favorite_delete)
        .on(MeRecentCreate, recent_create)
        .on(MeItemFavoriteSet, item_favorite_set)
        .on(MeItemFavoriteDelete, item_favorite_delete)
        .on(MeVisitCreate, visit_create)
        .on_snapshot_read_with_optional_operation(
            MeReadStateList,
            crate::key_operations::native::selected::<native::ReadStates>,
            read_state_list,
        )
        .on_snapshot_read_with_optional_operation(
            MeSubscriptionList,
            crate::key_operations::native::selected::<native::Subscriptions>,
            subscription_list,
        )
        .on(MeStateGet, state_get)
        .on(MeStateSet, state_set)
        .on(MePickerRecentsGet, picker_recents_get)
        .on(MePickerRecentCreate, picker_recent_create)
        .on(MeModelRecentList, model_recent_list)
        .on(MeModelRecentCreate, model_recent_create)
        .on(MeProjectViewSet, project_view_set)
        .on(MeProjectViewDelete, project_view_delete);
}
orch_api_base::__api_bind_3346c39c88!();

fn creation_ack<B: Send + 'static>() -> crate::middleware::idempotency::ObservedCreation<(), B, ()>
{
    crate::middleware::idempotency::ObservedCreation {
        kind: "recent_activity",
        replay: |ctx, (), _body, _created| {
            Box::pin(async move {
                crate::key_operations::publication::receipt(&ctx, &())?;
                Ok(())
            })
        },
    }
}

/// The caller's favorites in order, each with the way down to it, as far as
/// they may still view them.
async fn favorite_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Favorite>> {
    crate::key_operations::native::selected_read::<native::Favorites>(ctx, (), ()).await
}

/// Makes a node a favorite, or moves it within the list.
async fn favorite_set(ctx: Ctx, (): (), body: MeFavoriteSetBody) -> ApiResult<()> {
    let node = ctx.id("node_id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let user = ctx.user()?.id();
    let placing = match body.after_node_id {
        None => Placing::Last,
        Some(None) => Placing::First,
        Some(Some(after)) => Placing::After(after),
    };
    let view = match body.view {
        None => None,
        Some(None) => Some(None),
        Some(Some(view)) => {
            // A view is kept for a project alone: it is how its topics were looked at.
            let (_, kind) = placed(&mut conn, node).await?;
            if kind != NodeKind::Project {
                return Err(invalid("view", "is kept for a project alone"));
            }
            Some(Some(
                serde_json::to_value(view).map_err(ApiError::internal)?,
            ))
        }
    };
    if !hierarchy::put_favorite(&mut conn, user, node, placing, ctx.now).await? {
        return Err(invalid(
            "after_node_id",
            "is not one of the favorites it can go after",
        ));
    }
    // Left out, the view a favorite has stays; null stars the place alone.
    if let Some(view) = view {
        hierarchy::set_favorite_view(&mut conn, user, node, view.as_ref()).await?;
    }
    Ok(())
}

/// Takes a node out of the favorites.
async fn favorite_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let node = ctx.id("node_id")?;
    let mut conn = ctx.app.db.acquire().await?;
    hierarchy::delete_favorite(&mut conn, ctx.user()?.id(), node).await?;
    Ok(())
}

/// Notes that the caller went to a node they may view.
async fn recent_create(ctx: Ctx, (): (), body: MeRecentCreateBody) -> ApiResult<()> {
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard.principal();
    authorization::require(
        &mut conn,
        principal,
        body.node_id,
        VIEW,
        Act::Use,
        Policy::default(),
    )
    .await?;
    hierarchy::touch_recent(
        &mut conn,
        principal.user(),
        body.node_id,
        guard.now().get(),
        i64::from(ctx.app.config.limits.recents_per_user),
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

fn personal_target(ctx: &Ctx) -> ApiResult<PersonalTarget> {
    let kind = serde_json::from_value(Json::String(ctx.param("kind")?.to_owned()))
        .map_err(|_| invalid("kind", "is not a searchable kind"))?;
    Ok(PersonalTarget {
        kind,
        id: ctx.id("id")?,
    })
}

/// Exact identity lookups use the same bulk authorization as names, without searching for another person's data.
async fn require_target(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    target: &PersonalTarget,
    at: Time,
) -> ApiResult<()> {
    let ids = [target.id];
    let kinds = [target.kind.as_str().to_owned()];
    let candidates = orch_db::queries::search::names(
        conn,
        &orch_db::queries::search::Names {
            identifiers: Some(&ids),
            starred: false,
            picker_count: None,
            at: at.get(),
            user: principal.user(),
            trees: principal.member_of(),
            roots: &[],
            system_admin: principal.is_system_admin(),
            text: "",
            kinds: &kinds,
            org: None,
            offset: 0,
            limit: 1,
        },
    )
    .await?;
    let visible = orch_engine::projections::visible_names(conn, principal, &candidates)
        .await
        .map_err(ApiError::internal)?;
    if !visible.contains(&(target.kind.as_str().to_owned(), target.id)) {
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "that thing is not available",
        ));
    }
    Ok(())
}

async fn item_favorite_set(ctx: Ctx, (): (), body: MeItemFavoriteSetBody) -> ApiResult<()> {
    let target = personal_target(&ctx)?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require_target(&mut tx, guard.principal(), &target, guard.now()).await?;
    let placing = match &body.after {
        None => hierarchy::ItemPlacing::Last,
        Some(None) => hierarchy::ItemPlacing::First,
        Some(Some(after)) => hierarchy::ItemPlacing::After {
            kind: after.kind.as_str(),
            id: after.id,
        },
    };
    if !hierarchy::put_item_favorite(
        &mut tx,
        guard.principal().user(),
        target.kind.as_str(),
        target.id,
        placing,
        guard.now().get(),
    )
    .await?
    {
        return Err(invalid("after", "is not another favorite in your list"));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

async fn item_favorite_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let target = personal_target(&ctx)?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    hierarchy::delete_item_favorite(
        &mut tx,
        guard.principal().user(),
        target.kind.as_str(),
        target.id,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

async fn visit_create(ctx: Ctx, (): (), target: PersonalTarget) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    require_target(&mut tx, guard.principal(), &target, guard.now()).await?;
    hierarchy::touch_item_recent(
        &mut tx,
        guard.principal().user(),
        target.kind.as_str(),
        target.id,
        guard.now().get(),
        i64::from(ctx.app.config.limits.recents_per_user),
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// How far the caller has read their threads.
async fn read_state_list(
    ctx: Ctx,
    query: MeReadStateListQuery,
    (): (),
) -> ApiResult<Vec<ReadState>> {
    crate::key_operations::native::selected_read::<native::ReadStates>(ctx, query, ()).await
}

/// The caller's subscriptions.
async fn subscription_list(
    ctx: Ctx,
    query: MeSubscriptionListQuery,
    (): (),
) -> ApiResult<Vec<MeSubscription>> {
    crate::key_operations::native::selected_read::<native::Subscriptions>(ctx, query, ()).await
}

fn subscription(row: &hierarchy::SubscriptionRow) -> Option<MeSubscription> {
    Some(MeSubscription {
        node_id: row.node_id,
        level: serde_json::from_value(Json::String(row.level.clone())).ok()?,
        source: if row.source == "manual" {
            SubscriptionSource::Manual
        } else {
            SubscriptionSource::Auto
        },
    })
}

/// What the caller's interface remembers for them.
async fn state_get(ctx: Ctx, (): (), (): ()) -> ApiResult<PersonalState> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    Ok(identity::interface_state(&mut conn, principal.user())
        .await?
        .into_iter()
        .collect())
}

async fn model_recent_list(ctx: Ctx, (): (), (): ()) -> ApiResult<ModelRecents> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let recent_count = orch_settings::get::<i64>(
        &mut conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::INTERFACE_RECENT_MODELS,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::internal("the recent model default is missing"))?;
    let recent = identity::recent_models(&mut conn, principal.user())
        .await?
        .into_iter()
        .map(|(provider_id, model_name)| ModelRecent {
            provider_id,
            model_name,
        })
        .collect();
    Ok(ModelRecents {
        recent,
        recent_count,
    })
}

async fn model_recent_create(ctx: Ctx, (): (), body: ModelRecent) -> ApiResult<()> {
    if body.model_name.trim().is_empty() {
        return Err(invalid("model_name", "must name a model"));
    }
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let person = guard.principal().user();
    identity::note_model_choice(
        &mut conn,
        person,
        body.provider_id,
        &body.model_name,
        guard.now().get(),
    )
    .await?;
    orch_db::sequences::notify_user_event(
        &mut conn,
        &[person],
        &serde_json::json!({"what":"model_recents"}),
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Replaces the whole value of one key of that state.
async fn state_set(ctx: Ctx, (): (), mut value: Json) -> ApiResult<()> {
    let key = ctx.param("key")?;
    let known: Option<PersonalStateKey> = serde_json::from_value(Json::String(key.to_owned())).ok();
    if known.is_none() {
        return Err(invalid(
            "key",
            "is none of help, layouts, viewers, timeline, interface, sidebar, project_views, keys, reactions, picker_recents, inbox and thread_setup",
        ));
    }
    if known == Some(PersonalStateKey::PickerRecents) && value != serde_json::json!({}) {
        return Err(invalid(
            "picker_recents",
            "clear with an empty object; record selections through the picker choices request",
        ));
    }
    if known == Some(PersonalStateKey::Interface) {
        use orch_settings_contract::registry::{self, keys};
        let choices = value
            .as_object()
            .ok_or_else(|| invalid("interface", "must be an object"))?;
        for (field, setting) in [
            ("kept_closed_tabs", keys::INTERFACE_KEPT_CLOSED_TABS),
            ("progress_delay_ms", keys::INTERFACE_PROGRESS_DELAY_MS),
            ("catch_up_above_items", keys::INTERFACE_CATCH_UP_ABOVE_ITEMS),
            ("read_freshness_ms", keys::INTERFACE_READ_FRESHNESS_MS),
        ] {
            if let Some(choice) = choices.get(field) {
                registry::validate(setting, choice)
                    .map_err(|error| invalid(&format!("interface.{field}"), &error.to_string()))?;
            }
        }
    }
    // A person's keys are kept only as keys of actions there are, none of them meeting another's.
    if known == Some(PersonalStateKey::Keys) {
        let keys: ShortcutState = serde_json::from_value(value.clone())
            .map_err(|error| invalid("keys", &error.to_string()))?;
        keys.check()
            .map_err(|(member, why)| invalid(&member, &why))?;
    }
    let size = serde_json::to_vec(&value)
        .map_err(ApiError::internal)?
        .len();
    if i64::try_from(size).unwrap_or(i64::MAX) > PERSONAL_STATE_MAX_BYTES {
        return Err(
            ApiError::new(ErrorCode::RequestTooLarge, "the value is too large")
                .with("limit", PERSONAL_STATE_MAX_BYTES),
        );
    }
    if known == Some(PersonalStateKey::Reactions) {
        // The same glyph format as ordinary reactions; one platform grapheme, no custom-emoji
        // object or image. Preserve newest-first order while bounding distinct remembered choices.
        use unicode_segmentation::UnicodeSegmentation;
        const RECENT_REACTIONS: usize = 8;
        let entries = value
            .as_array()
            .ok_or_else(|| invalid("reactions", "must be a list of emoji"))?;
        let mut recent = Vec::new();
        for entry in entries {
            let text = entry
                .as_str()
                .ok_or_else(|| invalid("reactions", "each choice is an emoji"))?;
            let emoji = orch_documents::comments::clean_emoji(text)
                .map_err(|_| invalid("reactions", "each choice is one emoji"))?;
            if emoji.graphemes(true).count() != 1 {
                return Err(invalid("reactions", "each choice is one emoji"));
            }
            if recent.len() < RECENT_REACTIONS && !recent.contains(&emoji) {
                recent.push(emoji);
            }
        }
        value = serde_json::to_value(recent).map_err(ApiError::internal)?;
    }
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let user = guard.principal().user();
    identity::put_interface_state(&mut conn, user, key, &value, guard.now().get()).await?;
    orch_db::sequences::notify_user_event(
        &mut conn,
        &[user],
        &serde_json::json!({"what": "state", "key": key}),
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

async fn picker_recents_get(ctx: Ctx, (): (), (): ()) -> ApiResult<MePickerRecents> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    orch_api_base::picker_recents::read(&mut conn, principal.user()).await
}

async fn picker_recent_create(
    ctx: Ctx,
    (): (),
    body: MePickerRecentChoice,
) -> ApiResult<MePickerRecentReceipt> {
    if body.kind != "model"
        && serde_json::from_value::<orch_api_contract::search::NameKind>(Json::String(
            body.kind.clone(),
        ))
        .is_err()
    {
        return Err(invalid("kind", "is no resource or model kind"));
    }
    if body.key.trim().is_empty() {
        return Err(invalid("key", "is empty"));
    }
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let user = guard.principal().user();
    let count = orch_api_base::picker_recents::count(&mut conn).await?;
    if count == 0 {
        conn.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(MePickerRecentReceipt {
            count,
            remembered: false,
        });
    }
    identity::note_picker_choice(
        &mut conn,
        user,
        &body.kind,
        &body.key,
        i64::try_from(count).map_err(ApiError::internal)?,
        guard.now().get(),
    )
    .await?;
    orch_db::sequences::notify_user_event(
        &mut conn,
        &[user],
        &serde_json::json!({"what":"state","key":"picker_recents"}),
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(MePickerRecentReceipt {
        count,
        remembered: true,
    })
}

/// Keeps the caller's last view of one project they may view, beside the views of the others: two
/// devices that save the views of two projects at once keep both. The value as it then is holds to
/// the bound of a key, or nothing is written.
async fn project_view_set(ctx: Ctx, (): (), body: ProjectView) -> ApiResult<()> {
    let project = ctx.id("project_id")?;
    let mut conn = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (_, kind) = placed(&mut conn, project).await?;
    if kind != NodeKind::Project {
        return Err(ApiError::not_found());
    }
    authorization::require(
        &mut conn,
        &principal,
        project,
        VIEW,
        Act::Use,
        Policy::default(),
    )
    .await?;
    let view = serde_json::to_value(&body).map_err(ApiError::internal)?;
    let user = principal.user();
    let size = identity::put_project_view(&mut conn, user, project, &view, ctx.now).await?;
    if size > PERSONAL_STATE_MAX_BYTES {
        return Err(
            ApiError::new(ErrorCode::RequestTooLarge, "the value is too large")
                .with("limit", PERSONAL_STATE_MAX_BYTES),
        );
    }
    orch_db::sequences::notify_user_event(
        &mut conn,
        &[user],
        &serde_json::json!({"what": "state", "key": "project_views"}),
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Forgets the caller's last view of one project. It asks for nothing of the project: what one saved
/// for a place one can no longer see is one's own to clear.
async fn project_view_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let project = ctx.id("project_id")?;
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    let user = guard.principal().user();
    identity::delete_project_view(&mut conn, user, project, guard.now().get()).await?;
    orch_db::sequences::notify_user_event(
        &mut conn,
        &[user],
        &serde_json::json!({"what": "state", "key": "project_views"}),
    )
    .await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

#[path = "personal/viewed.rs"]
pub mod viewed;

#[path = "personal/native.rs"]
pub mod native;
