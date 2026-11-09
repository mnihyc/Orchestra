//! Native environment setting values and actual provenance labels on a current read snapshot.

use super::*;
use crate::key_operations::{native, targets};
use crate::registry::OperationPlan;
use crate::routes::access::page::NameBounds;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::settings_sources::{self as sources, Source};
use orch_authz::Caller;
use orch_settings::{Place as SettingsPlace, resolve_all};
use std::collections::BTreeSet;

pub(super) struct Environment;
pub(super) struct Selected {
    views: Vec<orch_settings::SettingView>,
    names: NameBounds,
}
const NAMES: &[(ObjectKind, Source)] = &[
    (ObjectKind::Org, Source::OrgNames),
    (ObjectKind::Project, Source::ProjectNames),
    (ObjectKind::Topic, Source::TopicNames),
    (ObjectKind::Thread, Source::ThreadNames),
];

impl native::Selected for Environment {
    type Source = Source;
    type Query = ();
    type Value = Selected;
    type Reply = Vec<settings_api::ResolvedSetting>;
    const DECLARATION: &'static Declaration<Source> = &sources::ENVIRONMENT;

    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _query: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            if !native::modern(caller) {
                let views = resolve_all(conn, SettingsPlace::Environment(id))
                    .await
                    .map_err(ApiError::internal)?;
                return Ok((
                    Selected {
                        views,
                        names: NameBounds::default(),
                    },
                    Vec::new(),
                ));
            }
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                },
            )
            .await?;
            let own = native::Facts::one(
                Source::Environment,
                KeyTargetKind::Object(ObjectKind::Environment),
                target,
            );
            let known = native::bind_known(ctx, Self::DECLARATION, std::slice::from_ref(&own))?;
            let principal = known.preparation_principal(ctx, conn, caller).await?;
            Self::require(ctx, conn, &principal).await?;
            let views = resolve_all(conn, SettingsPlace::Environment(id))
                .await
                .map_err(ApiError::internal)?;
            let ids = views
                .iter()
                .flat_map(|view| view.nodes())
                .collect::<BTreeSet<_>>();
            let graph =
                orch_authz::engine::Graph::load(conn, &ids.iter().copied().collect::<Vec<_>>())
                    .await
                    .map_err(authorization::restoration_error)?;
            let plan = OperationPlan::new(ctx.route, Self::DECLARATION.operation)?;
            let (names, allowed) =
                native::node_name_facts(&plan, Self::DECLARATION, caller, &graph, &ids, NAMES)?;
            let mut facts = vec![own];
            facts.extend(names);
            Ok((
                Selected {
                    views,
                    names: NameBounds {
                        limited: true,
                        nodes: allowed,
                        ..NameBounds::default()
                    },
                },
                facts,
            ))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            queries::environments(conn, &[id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            require(
                conn,
                principal,
                ResourceKind::Environment,
                id,
                Permission::EnvironmentView,
            )
            .await
        })
    }

    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Selected,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            let manages = resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: ctx.id("id")?,
                },
                Permission::EnvironmentManage,
                Policy::default(),
            )
            .await
            .map_err(authorization::restoration_error)?
            .is_allowed();
            let views = value
                .views
                .iter()
                .cloned()
                .map(|view| {
                    let writable = manages && view.def().set_at(Scope::Environment);
                    (view, writable)
                })
                .collect();
            crate::routes::settings::readers::settings_shown(conn, principal, views, &value.names)
                .await
        })
    }
}
