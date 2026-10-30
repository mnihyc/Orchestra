//! What a change of a page, of the defaults of a place or of the defaults of the system would do, said before anything is changed: how many pages it reaches and who would gain and lose what on them.
//!
//! The change is the access engine's own, made inside a transaction that is never committed, so
//! that a preview refuses what the change would refuse and leaves nothing behind. A person is named
//! as far as the caller may know them and counted otherwise. The page of a place is one page
//! and opens nothing inside it; the defaults of a place reach the pages inside it that follow them.

use super::subject;
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::admin::{admin_of, defaults::put_back_shipped};
use crate::routes::nodes::{invalid, placed, policy_at};
use crate::sign_in::origin;
use orch_access_contract::pages as stored;
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::access as api;
use orch_api_contract::nodes::NodeKind;
use orch_authz::pages::{self, ChangeError, Target};
use orch_authz::previews::{self, Before, Moved, Preview, Scope};
use orch_authz::{Policy, things};
use orch_db::queries::access;
use orch_db::{Conn, DbError};
use orch_types::Id;

/// The preview as the API words it, with the people by name.
pub(crate) async fn worded(conn: &mut Conn, preview: Preview) -> ApiResult<api::PagePreview> {
    let ids: Vec<Id> = preview
        .gaining
        .iter()
        .chain(&preview.losing)
        .map(|moved| moved.user)
        .collect();
    let people = refusals::user_refs(conn, &ids).await?;
    let person = |moved: &Moved| -> Option<api::PreviewPerson> {
        Some(api::PreviewPerson {
            user: people.get(&moved.user)?.clone(),
            permissions: moved
                .permissions
                .iter()
                .map(|permission| permission.as_str().to_owned())
                .collect(),
            pages: moved.pages,
        })
    };
    let named_gaining: Vec<api::PreviewPerson> =
        preview.gaining.iter().filter_map(person).collect();
    let named_losing: Vec<api::PreviewPerson> = preview.losing.iter().filter_map(person).collect();
    // A person who is not a user any more cannot be named: counted with those who are not shown.
    let unnamed = i64::try_from(
        preview.gaining.len() + preview.losing.len() - named_gaining.len() - named_losing.len(),
    )
    .unwrap_or(i64::MAX);
    Ok(api::PagePreview {
        pages: preview.pages,
        orgs: preview.orgs,
        gaining: named_gaining,
        losing: named_losing,
        more: preview.more + unnamed,
    })
}

fn read_change(body: &api::PageChange) -> ApiResult<stored::PageChange> {
    crate::routes::notifications::person::page_change(body).map(|(change, _)| change)
}

/// What a change of the page of a node, a resource or a document would do.
pub async fn page(ctx: Ctx, (): (), body: api::PageChange) -> ApiResult<api::PagePreview> {
    let named = subject::named(&ctx)?;
    let change = read_change(&body)?;
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let (_, site) = things::site(&mut tx, named.thing)
        .await
        .map_err(|error| match error {
            orch_authz::Error::NotFound(_) => ApiError::not_found(),
            other => ApiError::internal(other),
        })?;
    let policy = subject::resource_policy(&mut tx, named.thing, &site).await?;
    let previewed = previews::of_page(
        &mut tx,
        &principal,
        named.thing,
        &change,
        &policy,
        &origin(&ctx),
    )
    .await;
    let answer = answered(&mut tx, previewed).await;
    // Nothing a preview did is kept: the change was made in a trial, and the transaction is ended
    // without being committed.
    tx.rollback().await.map_err(DbError::from)?;
    answer
}

/// What a change of what a place gives to what is made inside it would do.
pub async fn node_defaults(ctx: Ctx, (): (), body: api::PageChange) -> ApiResult<api::PagePreview> {
    let id = ctx.id("id")?;
    let change = read_change(&body)?;
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let (_, kind) = placed(&mut tx, id).await?;
    if kind == NodeKind::Thread {
        return Err(invalid(
            "id",
            "a thread has no defaults: it gives nothing to what is made inside it",
        ));
    }
    let policy = policy_at(&mut tx, id).await?;
    let previewed = previews::of_defaults(
        &mut tx,
        &principal,
        PlaceRef::Node(id),
        &change,
        policy,
        &origin(&ctx),
    )
    .await;
    let answer = answered(&mut tx, previewed).await;
    // Nothing a preview did is kept: the change was made in a trial, and the transaction is ended
    // without being committed.
    tx.rollback().await.map_err(DbError::from)?;
    answer
}

/// What a change of the defaults of the system, which every org starts from, would do. Putting
/// them back to what Orch ships is previewed as it is done: to those entries.
pub async fn admin_defaults(
    ctx: Ctx,
    (): (),
    body: api::PageChange,
) -> ApiResult<api::PagePreview> {
    admin_of(&ctx)?;
    let change = read_change(&body)?;
    let resets = change.whole == Some(stored::Whole::Reset);
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let before = Before::take(
        &mut tx,
        &principal,
        Scope::Inside(PlaceRef::System),
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let mut trial = access::begin_trial(&mut tx).await?;
    let previewed: Result<Preview, ChangeError> = async {
        pages::change(
            &mut trial,
            &principal,
            Target::Defaults(PlaceRef::System),
            &change,
            Policy::default(),
            &origin(&ctx),
        )
        .await?;
        if resets {
            put_back_shipped(&mut trial, &ctx).await.map_err(|error| {
                ChangeError::Engine(orch_authz::Error::Stored(error.to_string()))
            })?;
        }
        Ok(before.finish(&mut trial, &principal).await?)
    }
    .await;
    access::end_trial(trial).await?;
    let answer = answered(&mut tx, previewed).await;
    // Nothing a preview did is kept: the change was made in a trial, and the transaction is ended
    // without being committed.
    tx.rollback().await.map_err(DbError::from)?;
    answer
}

/// The answer to a preview.
async fn answered(
    conn: &mut Conn,
    previewed: Result<Preview, ChangeError>,
) -> ApiResult<api::PagePreview> {
    match previewed {
        Ok(preview) => worded(conn, preview).await,
        Err(error) => Err(refusals::change_failed(conn, error).await),
    }
}
