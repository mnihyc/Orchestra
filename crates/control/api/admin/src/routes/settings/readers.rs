//! Native settings and secret metadata prepared on a caller-supplied read snapshot.
//!
//! Preparation keeps raw resolver facts. The enclosing operation admits the actual
//! provenance sources before these helpers put bounded current names in the response.

use super::add_automation_reason;

use super::add_mode_choices;

use super::secret_failed;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::same;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::settings as api;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_settings::secrets;

use orch_settings::secrets::Owner;

use orch_settings::Place;

use orch_settings::SettingView;

use orch_types::Id;

use orch_types::Time;

pub use orch_api_base::routes::settings::readers::PreparedSettings;

pub use orch_api_base::routes::settings::readers::PreparedSecrets;

pub use orch_api_base::routes::settings::readers::resolved_views;
/// The native readers' existing NodeView decision, on this operation's current Principal.
/// The caller owns credential restoration and operation admission on the supplied snapshot;
/// no child endpoint selection, replacement caller or wide-name proof is created here.
async fn readable(conn: &mut Conn, principal: &Principal, node: Id) -> ApiResult<()> {
    let bounded = principal
        .caller()
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some());
    authorization::require_projected(
        conn,
        principal,
        node,
        authorization::VIEW,
        Act::Use,
        Policy::default(),
        bounded,
    )
    .await
}

/// Resolve the same values, locks and writability hints as the native node settings list.
pub(crate) async fn prepare_settings(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
) -> ApiResult<PreparedSettings> {
    readable(conn, principal, node).await?;
    Ok(PreparedSettings {
        node,
        views: resolved_views(conn, principal, node, None).await?,
    })
}
orch_api_base::__api_bind_e928a69f70!();

/// Project once, after the enclosing operation has bound the actual provenance names.
/// Existing derived mode choices use their native internal Use/capability checks.
pub(crate) async fn show_settings(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedSettings,
    bounds: &NameBounds,
) -> ApiResult<Vec<api::ResolvedSetting>> {
    show_settings_with_controls(ctx, conn, principal, prepared, bounds, true).await
}
orch_api_base::__api_bind_63a3158c5e!();

/// The enclosing reader may omit descriptor-derived choices whose resource sources it cannot read.
/// Effective values and native writability hints are projected by the same settings producer.
pub(crate) async fn show_settings_with_controls(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedSettings,
    bounds: &NameBounds,
    offer_modes: bool,
) -> ApiResult<Vec<api::ResolvedSetting>> {
    let mut rows = settings_shown(conn, principal, prepared.views.clone(), bounds).await?;
    add_automation_reason(conn, prepared.node, &mut rows).await?;
    if offer_modes {
        add_mode_choices(ctx, conn, principal, Place::Node(prepared.node), &mut rows).await?;
    }
    Ok(rows)
}
orch_api_base::__api_bind_645b0cf6ac!();

fn setting_sources(views: &[(SettingView, bool)]) -> Vec<Id> {
    let mut ids: Vec<_> = views.iter().flat_map(|(view, _)| view.nodes()).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}
orch_api_base::__api_bind_29632770a2!();

/// One projector also serves existing write and selected-key responses.
pub(crate) async fn settings_shown(
    conn: &mut Conn,
    principal: &Principal,
    views: Vec<(SettingView, bool)>,
    bounds: &NameBounds,
) -> ApiResult<Vec<api::ResolvedSetting>> {
    let refs = bounds
        .refs(conn, principal, &setting_sources(&views))
        .await?;
    views
        .into_iter()
        .map(|(view, permitted)| {
            let setting = view.into_setting(permitted).map_node(|id| {
                refs.get(&id).cloned().unwrap_or(NodeRef {
                    thread_display: None,
                    id,
                    kind: NodeKind::Org,
                    name: None,
                    redacted: true,
                })
            });
            same(&setting).ok_or_else(|| ApiError::internal("a setting does not read as the API's"))
        })
        .collect()
}
orch_api_base::__api_bind_e4bc8ef338!();

/// Read metadata through the existing hierarchy resolver, without opening sealed values.
pub(crate) async fn prepare_secrets(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
) -> ApiResult<PreparedSecrets> {
    readable(conn, principal, node).await?;
    let list = match secrets::list(conn, Owner::Node(node)).await {
        Ok(list) => list,
        Err(error) => return Err(secret_failed(conn, node, error).await),
    };
    Ok(PreparedSecrets { list })
}
orch_api_base::__api_bind_b00f576e8e!();

pub(crate) async fn show_secrets(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedSecrets,
    bounds: &NameBounds,
) -> ApiResult<Vec<api::SecretInfo>> {
    secrets_shown(conn, principal, prepared.list.clone(), bounds).await
}
orch_api_base::__api_bind_d846652dcd!();

pub(super) async fn secrets_shown(
    conn: &mut Conn,
    principal: &Principal,
    list: Vec<secrets::SecretInfo>,
    bounds: &NameBounds,
) -> ApiResult<Vec<api::SecretInfo>> {
    let ids: Vec<Id> = list.iter().filter_map(|info| info.node).collect();
    let refs = bounds.refs(conn, principal, &ids).await?;
    Ok(list
        .into_iter()
        .map(|info| api::SecretInfo {
            name: info.name,
            node: info.node.and_then(|node| refs.get(&node).cloned()),
            rotated_at: Some(Time::new(info.rotated_at.unwrap_or(info.created_at))),
        })
        .collect())
}
