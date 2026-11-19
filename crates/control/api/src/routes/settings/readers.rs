//! Native settings and secret metadata prepared on a caller-supplied read snapshot.
//!
//! Preparation keeps raw resolver facts. The enclosing operation admits the actual
//! provenance sources before these helpers put bounded current names in the response.

use super::{
    Holds, add_automation_reason, add_mode_choices, placed, policy_at, scope_of, secret_failed,
};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::access::page::{NameBounds, same};
use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_api_contract::settings as api;
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_settings::secrets::{self, Owner};
use orch_settings::validation;
use orch_settings::{Place, SettingView, resolve_all};
use orch_types::{Id, Time};

/// Raw resolved values and write hints, without a projected Node reference.
#[derive(Clone)]
pub(crate) struct PreparedSettings {
    node: Id,
    views: Vec<(SettingView, bool)>,
}

impl PreparedSettings {
    /// Only actual provenance references, never identifiers guessed from stored values.
    pub(crate) fn source_nodes(&self) -> Vec<Id> {
        setting_sources(&self.views)
    }

    /// A configured resource identity already present in the settings' stored value.
    pub(crate) fn selected_id(&self, key: &str, member: Option<&str>) -> Option<Id> {
        let value = self
            .views
            .iter()
            .find(|(view, _)| view.key() == key)?
            .0
            .value()?;
        let value = match member {
            Some(member) => value.get(member)?,
            None => value,
        };
        value.as_str()?.parse().ok()
    }
}

/// Secret names and timestamps only. This type never contains a secret value.
#[derive(Clone)]
pub(crate) struct PreparedSecrets {
    list: Vec<secrets::SecretInfo>,
}

impl PreparedSecrets {
    pub(crate) fn source_nodes(&self) -> Vec<Id> {
        let mut ids: Vec<_> = self.list.iter().filter_map(|info| info.node).collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

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

/// Kept separate from the list's NodeView gate: existing internal selected-key readers
/// already have their own ordinary entry gate and retain that exact behavior.
pub(crate) async fn resolved_views(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    keys: Option<&[&str]>,
) -> ApiResult<Vec<(SettingView, bool)>> {
    let (_, kind) = placed(conn, node).await?;
    let policy = policy_at(conn, node).await?;
    let holds = Holds::of(conn, principal, node, kind, policy).await?;
    let views = resolve_all(conn, Place::Node(node))
        .await
        .map_err(ApiError::internal)?;
    let scope = scope_of(kind);
    Ok(views
        .into_iter()
        .filter(|view| keys.is_none_or(|keys| keys.contains(&view.def().key)))
        .map(|view| {
            let need =
                validation::need_for_node(view.def(), scope, node, holds.thread_environment());
            let permitted = need.is_some_and(|need| holds.meets(need));
            (view, permitted)
        })
        .collect())
}

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

fn setting_sources(views: &[(SettingView, bool)]) -> Vec<Id> {
    let mut ids: Vec<_> = views.iter().flat_map(|(view, _)| view.nodes()).collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

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

pub(crate) async fn show_secrets(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedSecrets,
    bounds: &NameBounds,
) -> ApiResult<Vec<api::SecretInfo>> {
    secrets_shown(conn, principal, prepared.list.clone(), bounds).await
}

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
