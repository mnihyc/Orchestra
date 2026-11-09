//! The native administrative index request, retained beside the index rather than in settings.
//! Each short database act restores the exact operation; model IO holds no authority transaction.

use super::choice::{Choice, Embeddings, Tuning, choice};
use crate::EngineError;
use crate::operation_scope::{
    continuing::{self, Error, FirstEffect, Source},
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_api_contract::admin::{AdminSemanticSearchKeep, AdminSemanticSearchRebuild};
use orch_api_contract::admin_sources::{self, Source as Role};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::routes::{Endpoint, Route};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::{Conn, DbError, queries::semantic as rows};
use orch_types::{Clock, Id, Time, new_id};
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Model {
    provider: Id,
    name: String,
    dimensions: Option<u32>,
    query: String,
    document: String,
    writer: Option<Id>,
}
impl From<&Embeddings> for Model {
    fn from(value: &Embeddings) -> Self {
        Self {
            provider: value.provider_id,
            name: value.model_name.clone(),
            dimensions: value.dimensions,
            query: value.query_prefix.clone(),
            document: value.document_prefix.clone(),
            writer: value.writer,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Configuration {
    enabled: bool,
    model: Option<Model>,
    disk_share: Option<i64>,
    tuning: Tuning,
}
impl From<&Choice> for Configuration {
    fn from(value: &Choice) -> Self {
        Self {
            enabled: value.enabled,
            model: value.embeddings.as_ref().map(Model::from),
            disk_share: value.max_disk_share,
            tuning: value.tuning,
        }
    }
}

/// Server-private provenance of one native administrative choice, never a reusable admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    id: Id,
    org: Id,
    origin: CapturedOperation,
    configuration: Configuration,
    revisions: Vec<(Option<Id>, String, i64)>,
}
impl Request {
    /// Capture only after the shared operation admitted the native writer's actual operands.
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        chosen: &Choice,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        if !matches!(
            origin.operation(),
            "AdminSemanticSearchKeep" | "AdminSemanticSearchRebuild"
        ) {
            return Err(EngineError::unreadable(
                "not an administrative index choice",
            ));
        }
        Ok(Self {
            id: new_id(),
            org: chosen.org,
            origin,
            configuration: Configuration::from(chosen),
            revisions: revisions(conn, chosen.org).await?,
        })
    }
    /// The private request identity also marks each generation produced by this request.
    #[must_use]
    pub fn id(&self) -> Id {
        self.id
    }
    /// Retain in the same native transaction as the actual settings/generation effect.
    pub async fn store(&self, conn: &mut Conn, at: Time) -> Result<(), EngineError> {
        let value = serde_json::to_value(self).map_err(EngineError::unreadable)?;
        sqlx::query("INSERT INTO search_indexes(org_id,administration,updated_at) VALUES($1,$2,$3)
            ON CONFLICT(org_id) DO UPDATE SET administration=EXCLUDED.administration,updated_at=EXCLUDED.updated_at")
            .bind(self.org).bind(value).bind(at.get()).execute(conn).await.map_err(DbError::from)?;
        Ok(())
    }
}

/// Actual immutable native generation association, independent of the org's scope governor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Generation {
    id: Id,
    org: Id,
    request: Option<Id>,
    provider: Id,
    model: String,
    fingerprint: Vec<u8>,
    chunking: String,
    query: String,
    document: String,
    dimensions: Option<i32>,
    length: Option<i32>,
    writer: Option<Id>,
    created_at: chrono::DateTime<chrono::Utc>,
}
impl From<&rows::GenerationRow> for Generation {
    fn from(value: &rows::GenerationRow) -> Self {
        Self {
            id: value.id,
            org: value.org_id,
            request: value.administration_id,
            provider: value.provider_id,
            model: value.model_name.clone(),
            fingerprint: value.provider_fingerprint.clone(),
            chunking: value.chunking.clone(),
            query: value.query_prefix.clone(),
            document: value.document_prefix.clone(),
            dimensions: value.asked_dimensions,
            length: value.dimensions,
            writer: value.writer_id,
            created_at: value.created_at,
        }
    }
}

/// A particular native turn or generation; no HTTP context or account-only reconstruction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Work {
    request: Request,
    generation: Option<Generation>,
}
impl Work {
    /// Read the durable native request. Malformed/present evidence never becomes ordinary work.
    pub async fn load(conn: &mut Conn, org: Id) -> Result<Option<Self>, EngineError> {
        let value: Option<serde_json::Value> =
            sqlx::query_scalar("SELECT administration FROM search_indexes WHERE org_id=$1")
                .bind(org)
                .fetch_optional(conn)
                .await
                .map_err(DbError::from)?
                .flatten();
        value
            .map(|value| {
                let request: Request =
                    serde_json::from_value(value).map_err(EngineError::unreadable)?;
                if request.org != org {
                    return Err(EngineError::unreadable(
                        "the administrative index request moved",
                    ));
                }
                Ok(Self {
                    request,
                    generation: None,
                })
            })
            .transpose()
    }
    /// A genuinely changed native setting starts another turn. The old turn keeps its Work
    /// and cannot borrow that replacement; key revocation alone never starts ordinary work.
    pub async fn for_choice(conn: &mut Conn, chosen: &Choice) -> Result<Option<Self>, EngineError> {
        let first = revisions(conn, chosen.org).await?;
        let actual = choice(conn, chosen.org).await?;
        let last = revisions(conn, chosen.org).await?;
        if first != last || &actual != chosen {
            return Err(EngineError::unreadable(
                "the native index choice changed during preparation",
            ));
        }
        let work = Self::load(conn, chosen.org).await?;
        if let Some(work) = &work
            && last != work.request.revisions
        {
            return Ok(None);
        }
        Ok(work)
    }
    /// The real generation selected by the indexer, never another generation at the same org.
    #[must_use]
    pub fn generation(&self, row: &rows::GenerationRow) -> Self {
        Self {
            request: self.request.clone(),
            generation: Some(Generation::from(row)),
        }
    }
    /// The actual root org of this native turn.
    #[must_use]
    pub fn org(&self) -> Id {
        self.request.org
    }
    /// Generation creation retains its actual request association in a private column.
    #[must_use]
    pub fn request_id(&self) -> Id {
        self.request.id
    }
    /// Restore the canonical operation and run the native source factory after its real locks.
    pub async fn checkpoint(&self, conn: &mut Conn, clock: &dyn Clock) -> Result<Checked, Error> {
        match self.request.origin.operation() {
            "AdminSemanticSearchKeep" => {
                let parts = continuing::prepare::<Native<Keep>>(
                    conn,
                    clock,
                    &self.request.origin,
                    self.clone(),
                )
                .await?
                .into_parts();
                Ok(Checked {
                    caller: parts.principal.caller().clone(),
                    before: parts.first_effect,
                })
            }
            "AdminSemanticSearchRebuild" => {
                let parts = continuing::prepare::<Native<Rebuild>>(
                    conn,
                    clock,
                    &self.request.origin,
                    self.clone(),
                )
                .await?
                .into_parts();
                Ok(Checked {
                    caller: parts.principal.caller().clone(),
                    before: parts.first_effect,
                })
            }
            _ => Err(Error::Refused),
        }
    }
}

/// Current original caller plus the linear cut immediately before the native SQL/dispatch act.
pub struct Checked {
    /// Retained through the dispatcher's later currentness and live deadline checks.
    pub caller: Caller,
    /// Consumed before the actual native effect on this same still-open transaction.
    pub before: FirstEffect,
}

/// Check a native mutation after its own final preparation wait; ordinary work stays ordinary.
pub(super) async fn before(
    work: Option<&Work>,
    conn: &mut Conn,
    clock: &dyn Clock,
) -> Result<(), EngineError> {
    if let Some(work) = work {
        work.checkpoint(conn, clock)
            .await
            .map_err(EngineError::unreadable)?
            .before
            .check(clock)
            .map_err(EngineError::unreadable)?;
    }
    Ok(())
}

/// Private evidence attached to the actual embedding command, including its canonical bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingCommand {
    work: Work,
    canonical: serde_json::Value,
    provider: Id,
    writer: Id,
    model: String,
}
impl EmbeddingCommand {
    pub(super) fn capture(
        work: &Work,
        command: &crate::commands::Outgoing,
        named: &Embeddings,
        writer: Id,
    ) -> Result<Self, EngineError> {
        let body: orch_link_contract::commands::ProviderEmbed =
            serde_json::from_value(command.body.clone())?;
        if command.kind != orch_link_contract::commands::CommandKind::ProviderEmbed
            || command.key != orch_link_contract::commands::SerialKey::None
            || body.provider_id != named.provider_id
            || body.model_name != named.model_name
            || body.dimensions != named.dimensions
            || body.protocol != orch_model_contract::protocol::EMBEDDINGS
        {
            return Err(EngineError::unreadable(
                "the embedding command differs from its native provider and model",
            ));
        }
        Ok(Self {
            work: work.clone(),
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
            provider: named.provider_id,
            writer,
            model: named.model_name.clone(),
        })
    }
    /// The dispatcher calls this again after its actual queue/coordinator waits and on resend.
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue_callers(
            conn,
            &orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            clock,
            access,
        )
        .await
    }

    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if crate::jobs::command_identity::canonical(
            command.id,
            command.machine_id,
            command.serial_key,
            command.kind,
            command.body,
        )? != self.canonical
            || command.kind != "provider.embed"
        {
            return Ok(None);
        }
        let call: Option<(Id, Id, Option<Id>, String)> = sqlx::query_as(
            "SELECT node_id,provider_id,person_id,model_name FROM summary_calls WHERE id=$1 AND purpose='embeddings' AND cancel_reason IS NULL",
        ).bind(command.id).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
        if call.as_ref()
            != Some(&(
                self.work.org(),
                self.provider,
                Some(self.writer),
                self.model.clone(),
            ))
        {
            return Ok(None);
        }
        sqlx::query("SELECT id FROM providers WHERE id=$1 FOR SHARE")
            .bind(self.provider)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        let Some(provider) =
            orch_db::queries::resources::thread_engine::provider(conn, self.provider).await?
        else {
            return Ok(None);
        };
        if provider.deleted_at.is_some()
            || provider.key_machine_id != Some(command.machine_id)
            || !crate::providers::request_status_available(&provider.status)
            || rows::embedding_pause_reason(conn, command.id, self.provider)
                .await?
                .is_some()
            || !orch_db::queries::resources::thread_engine::model(conn, self.provider, &self.model)
                .await?
                .is_some_and(|model| model.enabled)
        {
            return Ok(None);
        }
        if !access
            .background_provider_use(
                conn,
                &crate::access::ProviderUse {
                    person: self.writer,
                    node: self.work.org(),
                    provider: self.provider,
                    model: Some(&self.model),
                    effort: None,
                },
            )
            .await?
            .is_allowed()
        {
            return Ok(None);
        }
        match self.work.checkpoint(conn, clock).await {
            Ok(current) => {
                current
                    .before
                    .check(clock)
                    .map_err(EngineError::unreadable)?;
                Ok(Some(vec![(Some(current.caller), None)]))
            }
            Err(Error::Refused) => Ok(None),
            Err(error) => Err(EngineError::unreadable(error)),
        }
    }
}

async fn revisions(
    conn: &mut Conn,
    org: Id,
) -> Result<Vec<(Option<Id>, String, i64)>, EngineError> {
    use orch_settings_contract::registry::keys;
    let mut rows = orch_db::queries::settings::rows_on_chain(
        conn,
        &[org],
        Some(&[
            keys::SEARCH_SEMANTIC_ENABLED.to_owned(),
            keys::SEARCH_SEMANTIC_EMBEDDINGS.to_owned(),
            keys::SEARCH_SEMANTIC_MAX_DISK_SHARE.to_owned(),
            keys::SEARCH_SEMANTIC_TUNING.to_owned(),
        ]),
    )
    .await?
    .into_iter()
    .map(|row| (row.node_id, row.key, row.revision))
    .collect::<Vec<_>>();
    rows.sort();
    Ok(rows)
}

trait Mode: Send + Sync + 'static {
    const ROUTE: Route;
    const ROW: &'static Declaration<Role>;
}
struct Keep;
impl Mode for Keep {
    const ROUTE: Route = AdminSemanticSearchKeep::ROUTE;
    const ROW: &'static Declaration<Role> = &admin_sources::SEMANTIC_KEEP;
}
struct Rebuild;
impl Mode for Rebuild {
    const ROUTE: Route = AdminSemanticSearchRebuild::ROUTE;
    const ROW: &'static Declaration<Role> = &admin_sources::SEMANTIC_REBUILD;
}
struct Native<M>(PhantomData<M>);
struct Value {
    request: Request,
    generation: Option<Generation>,
}
impl<M: Mode> Source for Native<M> {
    type Role = Role;
    type Input = Work;
    type Value = Value;
    const ROUTE: Route = M::ROUTE;
    const DECLARATION: &'static Declaration<Role> = M::ROW;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a Work,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let fresh = Work::load(conn, input.request.org)
                .await?
                .ok_or(Error::Refused)?;
            if fresh.request != input.request
                || rows::root_org(conn, input.request.org).await?.is_none()
                || Configuration::from(&choice(conn, input.request.org).await?)
                    != input.request.configuration
                || revisions(conn, input.request.org).await? != input.request.revisions
            {
                return Err(Error::Refused);
            }
            let generation = if let Some(old) = &input.generation {
                let row = rows::generations(conn, input.request.org)
                    .await?
                    .into_iter()
                    .find(|row| row.id == old.id)
                    .ok_or(Error::Refused)?;
                let current = Generation::from(&row);
                if current != *old || row.administration_id != Some(input.request.id) {
                    return Err(Error::Refused);
                }
                Some(current)
            } else {
                None
            };
            let target = targets::node(conn, input.request.org)
                .await
                .map_err(EngineError::unreadable)?;
            Ok((
                Value {
                    request: fresh.request,
                    generation,
                },
                vec![Facts::one(Role::Org, target.object.kind, target)],
            ))
        })
    }
    fn require<'a>(
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Work,
        _value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            // This is the existing native administrative act. Exact canonical source/action,
            // current credentials and second-step are admitted by the shared frame first.
            let modern = principal.caller().grant().is_some_and(|grant| {
                grant.operation_scope().is_some() && !grant.requires_legacy_compatibility()
            });
            if principal.has_system_admin_standing() && (modern || principal.is_system_admin()) {
                Ok(())
            } else {
                Err(Error::Refused)
            }
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Work,
        _value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR SHARE")
                .bind(input.request.org)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT org_id FROM search_indexes WHERE org_id=$1 FOR UPDATE")
                .bind(input.request.org)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
            if let Some(generation) = &input.generation {
                sqlx::query("SELECT id FROM search_generations WHERE id=$1 FOR UPDATE")
                    .bind(generation.id)
                    .fetch_one(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Work,
        old: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(
            async move { Ok(old.request == fresh.request && old.generation == fresh.generation) },
        )
    }
}
