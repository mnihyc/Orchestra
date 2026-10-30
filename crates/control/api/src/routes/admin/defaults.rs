//! The defaults page of the system, which every org starts from: read, changed, and put back to what Orch ships.
//!
//! The page is the access engine's like any other: it is read and changed by the very functions
//! that serve the defaults page of an org, with the system as the place. Whoever holds it as shipped
//! holds a role's worth of permissions in an org that has not changed its own page; a row of it that
//! is locked decides that row for every org below, whatever they put on theirs.
//!
//! What a change would reach (the request that previews one) is not bound: the engine counts the
//! pages that follow a changed row only inside a place that is a node, and says nobody who would
//! gain or lose, so there is nothing yet to answer it with for the system as the place.

use super::{admin_of, record};
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::access::page::permission_page;
use crate::sign_in::origin;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::pages as stored;
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::access::{PageChange, PermissionPage};
use orch_api_contract::admin::{AdminDefaultsGet, AdminDefaultsUpdate};
use orch_authz::pages::{self, ChangeError, Target};
use orch_authz::{Policy, Principal, epoch};
use orch_db::queries::access::{self, NewEntry, PageKey, SYSTEM_ID};
use orch_db::{Conn, DbError};
use orch_types::{Time, new_id};
use serde_json::json;

/// Binds this module's requests: reading the page and changing it.
pub fn mount(registry: &mut Registry) {
    registry
        .on(AdminDefaultsGet, get)
        .on(AdminDefaultsUpdate, update);
}

/// The page as the engine reads it for the caller.
async fn read(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<PermissionPage> {
    let policy = crate::routes::access::subject::policy_at_place(conn, None).await?;
    let view = pages::read(conn, principal, Target::Defaults(PlaceRef::System), policy)
        .await
        .map_err(|error| ApiError::internal(error.to_string()))?;
    permission_page(conn, principal, &view, None, Time::new(ctx.now)).await
}

/// Reads the defaults page of the system.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<PermissionPage> {
    admin_of(&ctx)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    read(&ctx, &mut conn, &principal).await
}

/// Writes the entries Orch ships on the defaults page of the system, who holds what in an org that
/// has not changed its own page. Says how many.
pub(crate) async fn put_back_shipped(conn: &mut Conn, ctx: &Ctx) -> ApiResult<usize> {
    let page = PageKey::new("system", SYSTEM_ID, "defaults");
    let options = json!({});
    let shipped = stored::shipped();
    for each in &shipped {
        access::insert_entry(
            conn,
            &NewEntry {
                id: new_id(),
                page: &page,
                profile: each.profile.as_str(),
                permission: each.permission.as_str(),
                holder_kind: "role",
                holder_id: None,
                holder_role: Some(each.role.as_str()),
                options: &options,
                expires_at: None,
                note: "",
                source: "seed",
                created_by: None,
                at: ctx.now,
            },
        )
        .await?;
    }
    Ok(shipped.len())
}

/// Changes the defaults page of the system, or puts it back to what Orch ships.
async fn update(mut ctx: Ctx, (): (), body: PageChange) -> ApiResult<PermissionPage> {
    admin_of(&ctx)?;
    let (change, people) = crate::routes::notifications::person::page_change(&body)?;
    if !people.is_empty() {
        return Err(super::invalid(
            "notify",
            "is available on a thing's own permission page",
        ));
    }
    let resets = change.whole == Some(stored::Whole::Reset);
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let admin = guard.user().id();
    let principal = guard.principal();
    let changed = pages::change(
        &mut tx,
        principal,
        Target::Defaults(PlaceRef::System),
        &change,
        Policy::default(),
        &origin(&ctx),
    )
    .await;
    match changed {
        Ok(_) => {}
        Err(ChangeError::Stale { current }) => {
            let page = read(&ctx, &mut tx, principal).await?;
            return Err(
                refusals::change_failed(&mut tx, ChangeError::Stale { current })
                    .await
                    .with("current", page),
            );
        }
        Err(error) => return Err(refusals::change_failed(&mut tx, error).await),
    }
    if resets {
        // A page that follows nothing is not put back by removing its rows: that would leave every
        // org with nothing. What it goes back to is what ships.
        let made = put_back_shipped(&mut tx, &ctx).await?;
        epoch::bump(&mut tx).await.map_err(ApiError::internal)?;
        let who = guard.user().user.display_name.clone();
        record(
            &ctx,
            &mut tx,
            admin,
            AuditAction::PermissionRowChanged,
            None,
            &format!("{who} put the defaults of the system back to what Orch ships"),
            json!({ "entries": made }),
        )
        .await?;
    }
    let page = read(&ctx, &mut tx, principal).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(page)
}
