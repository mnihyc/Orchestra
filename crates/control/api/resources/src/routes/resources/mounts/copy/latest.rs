//! The original copy owner can reopen its durable progress without reading a native recipe.
use super::*;
use crate::key_operations::targets;
use crate::registry::OperationPlan;
use orch_access_contract::{
    key_scope::{KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_api_contract::operations::FacetUsage;
use std::collections::HashMap;

pub(super) struct Latest;
pub(super) struct Value {
    row: Option<jobs::JobRow>,
    names: HashMap<Id, String>,
}
impl native::Selected for Latest {
    type Source = Source;
    type Query = ();
    type Value = Value;
    type Reply = Option<orch_api_contract::workspace::Job>;
    const DECLARATION: &'static Declaration<Source> = &declarations::READ;
    const WATCH_ORDINARY: bool = true;
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mount = ctx.id("id")?;
            let mount_target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Mount,
                    id: mount,
                },
            )
            .await?;
            let mut facts = vec![native::Facts::one(
                Source::Mount,
                KeyTargetKind::Object(ObjectKind::Mount),
                mount_target.clone(),
            )];
            let id:Option<Id> = sqlx::query_scalar("SELECT id FROM jobs WHERE kind='mount_copy' AND created_by=$1 AND plan->>'mount_id'=$2 ORDER BY created_at DESC,id DESC LIMIT 1")
                .bind(caller.user()).bind(mount.to_string()).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
            let row = match id {
                Some(id) => jobs::job(conn, id).await?,
                None => None,
            };
            if let Some(row) = &row {
                let review: Review = serde_json::from_value(row.plan["copy"]["review"].clone())
                    .map_err(ApiError::internal)?;
                if review.source().mount != mount || row.created_by != Some(caller.user()) {
                    return Err(ApiError::not_found());
                }
            }
            facts.push(native::Facts::collection(
                Source::Job,
                row.as_ref()
                    .map(|row| {
                        (
                            KeyTargetKind::Identity(KeyIdentityKind::Job),
                            vec![
                                orch_api_base::routes::environments::jobs::subject::identity(
                                    row,
                                    Some(&mount_target),
                                ),
                            ],
                        )
                    })
                    .into_iter()
                    .collect(),
            ));
            let mut names = HashMap::new();
            let mut named = Vec::new();
            let plan = OperationPlan::new(ctx.route, declarations::READ.operation)?;
            if let Some(row) = &row {
                for (machine, name) in crate::routes::environments::jobs::machine_name_sources(
                    conn,
                    std::slice::from_ref(row),
                )
                .await?
                {
                    let identity = targets::machine_subject(conn, machine).await?;
                    if !native::modern(caller)
                        || native::permits_usage(
                            &plan,
                            &declarations::READ,
                            caller,
                            &Source::MachineNames,
                            FacetUsage::ResponseContent,
                            KeyTargetKind::Object(ObjectKind::Machine),
                            &identity,
                        )?
                    {
                        names.insert(machine, name);
                        named.push(identity);
                    }
                }
            }
            facts.push(native::Facts::collection(
                Source::MachineNames,
                if named.is_empty() {
                    vec![]
                } else {
                    vec![(KeyTargetKind::Object(ObjectKind::Machine), named)]
                },
            ));
            Ok((Value { row, names }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                ResourceKind::Mount,
                ctx.id("id")?,
                Permission::MountView,
            )
            .await
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            let mut names = HashMap::new();
            for (machine, name) in &value.names {
                if seen(ctx, conn, principal, *machine).await?.is_some() {
                    names.insert(*machine, name.clone());
                }
            }
            match &value.row {
                Some(row) => Ok(Some(
                    crate::routes::environments::jobs::view_with_machine_sources(
                        conn,
                        row,
                        Some(&names),
                    )
                    .await?,
                )),
                None => Ok(None),
            }
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a (),
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.names == fresh.names
                && match (&old.row, &fresh.row) {
                    (Some(old), Some(fresh)) => {
                        old.id == fresh.id && old.updated_at == fresh.updated_at
                    }
                    (None, None) => true,
                    _ => false,
                })
        })
    }
}
