//! The original copy owner can reopen its durable progress without reading a native recipe.
use super::*;
use crate::key_operations::targets;
use orch_access_contract::{
    key_scope::{KeyIdentityKind, KeyTargetKind},
    permissions::ObjectKind,
};

pub(super) struct Latest;
impl native::Selected for Latest {
    type Source = Source;
    type Query = ();
    type Value = Option<jobs::JobRow>;
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
            Ok((row, facts))
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
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            match value {
                Some(row) => Ok(Some(
                    crate::routes::environments::jobs::view(conn, row).await?,
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
            Ok(match (old, fresh) {
                (Some(old), Some(fresh)) => {
                    old.id == fresh.id && old.updated_at == fresh.updated_at
                }
                (None, None) => true,
                _ => false,
            })
        })
    }
}
