//! Individual document summary options through the ordinary native source/read/write boundary.

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::publication;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use futures::future::BoxFuture;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::summaries::DocumentSummaryOptionsGet;

use orch_api_contract::summaries::DocumentSummaryOptionsSet;

use orch_api_contract::summaries::SummaryOptionsBody;

use orch_api_contract::summaries::SummaryOptionsReceipt;

use orch_api_contract::summaries::SummaryOptionsView;

use orch_api_contract::summaries::SummaryTarget;

use orch_api_contract::summary_sources as declarations;

use orch_api_contract::summary_sources::Source;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::queries::documents;

use orch_db::queries::summaries;

use orch_engine::summaries::options;

use orch_engine::summaries::sources;

use orch_settings::Place;

use orch_settings::locks::Here;

use orch_settings::locks::lock_above;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Json;

use serde_json::json;
pub(super) fn mount(registry: &mut Registry) {
    registry.on_snapshot_read_with_optional_operation(
        DocumentSummaryOptionsGet,
        native::stored::<Reader, (), ()>,
        native::stored_read::<Reader>,
    );
    registry.on_with_operation(DocumentSummaryOptionsSet, effects::prepare::<Editor>, set);
}

fn source_error(error: sources::SourceError) -> ApiError {
    match error {
        sources::SourceError::Unavailable
        | sources::SourceError::Authz(orch_authz::Error::NotFound(_)) => ApiError::not_found(),
        other => ApiError::internal(other),
    }
}

async fn identity(conn: &mut Conn, id: Id) -> ApiResult<sources::Identity> {
    sources::identify(conn, SummaryTarget::Document { id })
        .await
        .map_err(source_error)
}

fn facts(value: &sources::Identity) -> Vec<native::Facts<Source>> {
    vec![native::Facts::one(
        Source::Document,
        KeyTargetKind::Object(ObjectKind::Document),
        value.fact().clone(),
    )]
}

async fn require(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    permission: Permission,
) -> ApiResult<()> {
    let result = authorization::require_document(
        conn,
        principal,
        id,
        permission,
        orch_authz::Policy::default(),
    )
    .await;
    result.map_err(|error| {
        if native::modern(principal.caller()) {
            authorization::problem_without_content(error)
        } else {
            error
        }
    })
}

async fn locked(conn: &mut Conn, place: Id) -> ApiResult<bool> {
    let view = orch_settings::resolve(conn, Place::Node(place), keys::SUMMARY_TARGETS)
        .await
        .map_err(ApiError::internal)?;
    Ok(lock_above(Here::Own, view.resolved.locked_by.as_ref()).is_some())
}

struct Read {
    identity: sources::Identity,
    policy: options::Policy,
}

struct Reader;

impl native::Stored for Reader {
    type Source = Source;
    type Value = Read;
    const ROWS: &'static [Declaration<Source>] = &[declarations::READ];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Read, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let identity = identity(conn, ctx.id("id")?).await?;
            let facts = facts(&identity);
            let policy = options::resolve(conn, &identity)
                .await
                .map_err(ApiError::engine)?;
            Ok((Read { identity, policy }, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Read,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            value.identity.target().id(),
            Permission::DocView,
        ))
    }
}

impl native::StoredRead for Reader {
    type Reply = SummaryOptionsView;
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Read,
    ) -> BoxFuture<'a, ApiResult<SummaryOptionsView>> {
        Box::pin(async move {
            let writable = value.policy.locked_by.is_none()
                && orch_authz::documents::check(
                    conn,
                    principal,
                    value.identity.target().id(),
                    Permission::DocEdit,
                    orch_authz::Policy::default(),
                )
                .await
                .map_err(ApiError::internal)?
                .is_allowed();
            Ok(SummaryOptionsView {
                target: value.identity.target(),
                options: value.policy.options.clone(),
                min_interval_secs: value.policy.min_interval_secs,
                own: value.policy.own.clone(),
                revision: value.policy.own_revision,
                writable,
                locked: value.policy.locked_by.is_some(),
            })
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        opened: &'a Read,
        fresh: &'a Read,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.identity.fact() == fresh.identity.fact()
                && opened.policy.fingerprint == fresh.policy.fingerprint)
        })
    }
}

struct Edit {
    identity: sources::Identity,
    row: documents::DocumentRow,
    options: Option<Json>,
    expected_revision: i64,
    current_revision: i64,
    changed: bool,
}

struct Editor;

impl effects::Effect for Editor {
    type Source = Source;
    type Query = ();
    type Body = SummaryOptionsBody;
    type Value = Edit;
    type Output = SummaryOptionsReceipt;
    fn declaration(_: &Edit) -> &'static Declaration<Source> {
        &declarations::WRITE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: SummaryOptionsBody,
    ) -> BoxFuture<'a, ApiResult<(Edit, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if body.expected_revision < 0 {
                return Err(ApiError::invalid(
                    "expected_revision",
                    "must not be negative",
                ));
            }
            let id = ctx.id("id")?;
            let first = identity(conn, id).await?;
            let known = native::bind_required(ctx, &declarations::WRITE, &facts(&first))?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(conn, &principal, id, Permission::DocEdit).await?;
            let row = summaries::lock_document_options(conn, id)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            let identity = identity(conn, id).await?;
            if locked(conn, identity.place()).await? {
                return Err(ApiError::new(
                    ErrorCode::AccessLocked,
                    "a lock above decides these summary options",
                ));
            }
            let options = body
                .options
                .as_ref()
                .map(serde_json::to_value)
                .transpose()
                .map_err(ApiError::internal)?;
            let cache = summaries::get(conn, identity.target(), None).await?;
            let current_revision = cache.as_ref().map_or(0, |cache| cache.options_revision);
            if current_revision != body.expected_revision {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the document summary options changed meanwhile",
                ));
            }
            let changed =
                cache.as_ref().and_then(|cache| cache.own_options.as_ref()) != options.as_ref();
            let facts = facts(&identity);
            Ok((
                Edit {
                    identity,
                    row,
                    options,
                    expected_revision: body.expected_revision,
                    current_revision,
                    changed,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Edit,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                value.identity.target().id(),
                Permission::DocEdit,
            )
            .await?;
            if locked(conn, value.identity.place()).await? {
                return Err(ApiError::new(
                    ErrorCode::AccessLocked,
                    "a lock above decides these summary options",
                ));
            }
            Ok(())
        })
    }
}

impl effects::Apply for Editor {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Edit,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<SummaryOptionsReceipt>>> {
        Box::pin(async move {
            let receipt = |revision| SummaryOptionsReceipt {
                projection: ReceiptProjection::Receipt,
                target: value.identity.target(),
                revision,
            };
            if !value.changed {
                return Ok(effects::Outcome::Unchanged(receipt(value.current_revision)));
            }
            before.check()?;
            let cache = summaries::ensure(conn, value.identity.target(), None, ctx.now)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let updated = summaries::configure(
                conn,
                cache.id,
                value.expected_revision,
                value.options.as_ref(),
                principal.user(),
                ctx.now,
            )
            .await?
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the document summary options changed meanwhile",
                )
            })?;
            let action = if value.options.is_some() {
                AuditAction::SettingsUpdated
            } else {
                AuditAction::SettingsReset
            };
            let mut entry = orch_db::audit::Entry::new(
                orch_db::audit::Actor::User(principal.user()),
                action.as_str(),
                "document",
                Some(value.row.id),
                "Summary options changed",
            );
            entry.history = action.is_history_line();
            entry.node_id = Some(value.row.node_id);
            entry.org_id = Some(value.row.org_id);
            entry.details = json!({"key":keys::SUMMARY_TARGETS,"target":"document","inherit":value.options.is_none(),"revision":updated.options_revision});
            orch_db::audit::append(conn, &entry, &crate::sign_in::origin(ctx), ctx.now).await?;
            Ok(effects::Outcome::Applied(receipt(updated.options_revision)))
        })
    }
}

async fn set(ctx: Ctx, (): (), _: SummaryOptionsBody) -> ApiResult<SummaryOptionsReceipt> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Editor>(&ctx, &mut tx).await?;
    guard.check_deadline(&done.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    publication::receipt(&done.ctx, &done.output)?;
    Ok(done.output)
}
