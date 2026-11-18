//! A delegated model step belongs to an actual retained Ask call and its still-current controller.
use super::{admission, command, dialogue::ProgramRequest, model};
use crate::{EngineError, inputs, operation_scope, unit::Unit};
use orch_access_contract::permissions::Permission;
use orch_api_contract::{
    ask::{AskMessageCreate, AskModel},
    routes::Endpoint,
    threads::ContentPart,
};
use orch_authz::{Act, Caller, Principal, http_origin::HttpPreparation};
use orch_db::{
    Conn, DbError,
    queries::{ask as rows, threads},
};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

/// Versions of the authorization scopes actually consulted to prepare a model request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadEpochs(std::collections::BTreeMap<Id, i64>);

impl ReadEpochs {
    pub async fn observe(conn: &mut Conn, nodes: &[Id]) -> Result<Self, EngineError> {
        let seen: Json = sqlx::query_scalar(
            "SELECT orch_authz_epochs(ARRAY(SELECT DISTINCT org_id FROM nodes WHERE id=ANY($1))) ||
                    orch_setting_json('orch.authz_seen','{}')",
        )
        .bind(nodes)
        .fetch_one(conn)
        .await
        .map_err(DbError::from)?;
        Ok(serde_json::from_value(seen)?)
    }

    pub fn include(&mut self, other: Self) {
        // Preserve the first read so a later concurrent change invalidates the whole request.
        for (root, version) in other.0 {
            self.0.entry(root).or_insert(version);
        }
    }

    pub async fn current(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        if self.0.is_empty() {
            return Ok(false);
        }
        let roots: Vec<_> = self.0.keys().copied().collect();
        Ok(
            sqlx::query_scalar("SELECT orch_authz_epochs($1::uuid[])=$2 AND orch_authz_saw($2)")
                .bind(roots)
                .bind(serde_json::to_value(self)?)
                .fetch_one(conn)
                .await
                .map_err(DbError::from)?,
        )
    }
}

/// Fingerprint the caller's current restrictions without carrying an operation proof into a later read.
pub fn credential_stamp(caller: &Caller) -> Result<orch_types::Hash, EngineError> {
    let mut restrictions =
        serde_json::to_value(orch_authz::tokens::CapturedCaller::capture(caller))?;
    restrictions.sort_all_objects();
    Ok(orch_types::Hash::of(&serde_json::to_vec(&restrictions)?))
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub(crate) call: Id,
    pub(crate) thread: Id,
    pub(crate) root: Id,
    pub(crate) turn: Id,
    segment: Id,
    controller: Id,
    pub(crate) person: Id,
    payer: Id,
    raw: HttpPreparation,
    pub(crate) request: ProgramRequest,
    pub(crate) deadline: Time,
}

fn ended() -> EngineError {
    EngineError::StaleToolSource
}

/// Preserve the actual admitted input operation, also for built-in turns entered through
/// the ordinary create, input and retry doors. The source restores it on each native effect.
pub fn input_origin(caller: &Caller) -> Result<Json, EngineError> {
    inputs::input_authority(caller)
}

impl Source {
    pub async fn capture(
        conn: &mut Conn,
        clock: &dyn Clock,
        owner: command::Controller,
        step: &super::Step,
        raw: HttpPreparation,
        repair: bool,
    ) -> Result<Self, EngineError> {
        let super::dialogue::Request::Program(request) = &step.request else {
            return Err(ended());
        };
        if repair != request.format.repair {
            return Err(ended());
        }
        let row = rows::get(conn, owner.thread, true)
            .await?
            .ok_or_else(ended)?;
        let at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT created_at FROM ask_calls WHERE id=$1 AND machine_id IS NULL AND NOT settled",
        )
        .bind(step.id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?
        .ok_or_else(ended)?;
        let deadline = Time::new(
            at + chrono::Duration::seconds(i64::from(
                orch_link_contract::commands::CommandKind::ProviderGenerate.deadline_secs(),
            )),
        );
        let source = Self {
            call: step.id,
            thread: owner.thread,
            turn: owner.turn,
            segment: step.segment,
            root: step.root,
            person: step.person,
            payer: row.payer_id.ok_or_else(ended)?,
            controller: owner.runner,
            deadline: raw.until().map_or(deadline, |until| until.min(deadline)),
            raw,
            request: request.clone(),
        };
        source.checked(conn, clock, true).await?.ok_or_else(ended)?;
        Ok(source)
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        rows::get(conn, self.thread, true)
            .await?
            .ok_or_else(ended)?;
        sqlx::query("SELECT id FROM ask_calls WHERE id=$1 FOR UPDATE")
            .bind(self.call)
            .fetch_optional(conn)
            .await
            .map_err(DbError::from)?
            .ok_or_else(ended)?;
        Ok(())
    }
    pub(crate) async fn callers(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<Caller>>, EngineError> {
        if orch_db::queries::transaction_guard::in_transaction(conn) {
            return self.checked(conn, clock, true).await;
        }
        let mut tx = sqlx::Connection::begin(conn).await.map_err(DbError::from)?;
        let callers = self.checked(&mut tx, clock, true).await?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(callers)
    }
    async fn checked(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        paid: bool,
    ) -> Result<Option<Vec<Caller>>, EngineError> {
        orch_db::queries::transaction_guard::lock_action(conn).await?;
        if clock.time() >= self.deadline || self.raw.user() != self.person {
            return Ok(None);
        }
        let Some(row) = rows::get(conn, self.thread, false).await? else {
            return Ok(None);
        };
        if row.root_org_id != self.root
            || row.turn != Some(self.turn)
            || row.segment != Some(self.segment)
            || row.runner != Some(self.controller)
            || row.payer_id != Some(self.payer)
            || row.state != "running"
            || row.temporary && row.user_id != self.person
            || !command::controller_alive(row.heartbeat, clock.now())
        {
            return Ok(None);
        }
        let Some(binding) = rows::segment_binding(conn, self.thread, self.segment).await? else {
            return Ok(None);
        };
        if !matches!(binding.transport,rows::DialogueTransport::Program(ref backend) if backend==&self.request.backend)
            || binding.model != self.request.model_name
        {
            return Ok(None);
        }
        let issued:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ask_calls WHERE id=$1 AND thread_id=$2 AND turn_id=$3 AND person_id=$4
            AND root_org_id=$5 AND provider_id=$6 AND machine_id IS NULL AND request=$7 AND NOT settled)")
            .bind(self.call).bind(self.thread).bind(self.turn).bind(self.person).bind(self.root).bind(binding.provider_id).bind(serde_json::to_value(&self.request)?)
            .fetch_one(&mut *conn).await.map_err(DbError::from)?;
        if !issued || !self.request.read_epochs.current(conn).await? {
            return Ok(None);
        }
        let Some(turn) = threads::turn(conn, self.turn).await? else {
            return Ok(None);
        };
        let Some(thread) = threads::thread(conn, self.thread).await? else {
            return Ok(None);
        };
        let Some(input) = threads::input(conn, turn.input_id).await? else {
            return Ok(None);
        };
        if turn.thread_id != self.thread
            || turn.segment_id != self.segment
            || turn.state != "running"
            || turn.interrupted_by.is_some()
            || input.thread_id != self.thread
            || input.turn_id != Some(self.turn)
            || input.author_id != Some(self.person)
            || input.run_as.is_some()
            || thread.deleted_at.is_some()
            || thread.archived_at.is_some()
        {
            return Ok(None);
        }
        let raw = self.raw.current(conn, clock).await.map_err(|_| ended())?;
        if credential_stamp(raw.caller())? != self.request.credential_stamp {
            return Ok(None);
        }
        let content: Vec<ContentPart> = serde_json::from_value(input.content.clone())?;
        let hashes = orch_chat::following::content_blobs(&content);
        for hash in &hashes {
            if rows::file(conn, self.thread, hash).await?.is_none() {
                return Ok(None);
            }
        }
        for part in self
            .request
            .input
            .messages
            .iter()
            .flat_map(|message| &message.content)
        {
            match part {
                orch_model_contract::generation::GenerationPart::ImageRef {
                    blob,
                    size,
                    media_type,
                    ..
                } => {
                    if rows::file(conn, self.thread, blob)
                        .await?
                        .is_none_or(|file| file.size != *size || file.media_type != *media_type)
                    {
                        return Ok(None);
                    }
                }
                orch_model_contract::generation::GenerationPart::Image { .. } => return Ok(None),
                _ => {}
            }
        }
        let caller = if self.raw.operation() == "AskMessageCreate" {
            let target = operation_scope::targets::node(conn, self.thread)
                .await
                .map_err(EngineError::unreadable)?;
            let plan = operation_scope::sources::bind_required(
                AskMessageCreate::ROUTE,
                &command::MESSAGE,
                &command::facts(target, !hashes.is_empty()),
            )
            .map_err(EngineError::unreadable)?;
            let Some(boundary) =
                inputs::ask_message_boundary(conn, &input, raw.caller(), clock).await?
            else {
                return Ok(None);
            };
            plan.admit(&boundary, false).map_err(|_| ended())?
        } else {
            let authority =
                inputs::saved_authority(input.submission_authority.as_ref())?.ok_or_else(ended)?;
            let mut unit = Unit::new(conn, clock.now());
            let Some(actors) = inputs::input_authority_callers(
                &mut unit,
                &thread,
                &input,
                self.person,
                &authority,
                clock,
            )
            .await?
            else {
                return Ok(None);
            };
            let [actor] = actors.callers() else {
                return Ok(None);
            };
            if actor
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
            {
                raw.caller()
                    .narrow_current_admission(actor)
                    .map_err(|_| ended())?
            } else {
                raw.caller()
                    .intersect_restrictions(actor)
                    .map_err(|_| ended())?
            }
        };
        let principal = Principal::load(conn, caller, clock.time())
            .await?
            .ordinary(conn)
            .await?;
        if !matches!(
            admission::root_access(conn, &principal, self.root).await?,
            admission::RootAccess::Allowed
        ) {
            return Ok(None);
        }
        let policy = admission::page_policy(conn, self.thread).await?;
        for permission in [Permission::ThreadView, Permission::ThreadInteract] {
            if !orch_authz::nodes::check(
                conn,
                &principal,
                self.thread,
                permission,
                Act::Use,
                policy,
            )
            .await?
            .is_allowed()
            {
                return Ok(None);
            }
        }
        if !paid {
            raw.check(clock).map_err(|_| ended())?;
            return Ok(Some(vec![principal.caller().clone()]));
        }
        let selected = match model::select(
            conn,
            &principal,
            self.root,
            AskModel {
                backend: self.request.backend.clone(),
                params: self.request.params.clone(),
            },
            clock.time(),
        )
        .await
        {
            Ok(selected) => selected,
            Err(model::Error::Denied(_)) => return Ok(None),
            Err(model::Error::Db(error)) => return Err(error.into()),
            Err(model::Error::Native(error)) => return Err(error),
        };
        if selected.payer != self.payer
            || selected.protocol.is_some()
            || selected.structured_output != Some(self.request.format.enforced)
            || selected.parameters != self.request.model_parameters
            || selected.model.params != self.request.params
        {
            return Ok(None);
        }
        if !crate::caps::retained_turn_room(
            conn,
            &crate::caps::OwnCapAsk {
                place: self.root,
                provider: binding.provider_id,
                model: &binding.model,
                tier: None,
                purpose: "ask",
                person: Some(self.person),
            },
            clock.time(),
            self.turn,
        )
        .await?
        {
            return Ok(None);
        }
        raw.check(clock).map_err(|_| ended())?;
        let mut callers = vec![principal.caller().clone()];
        if self.payer != self.person {
            callers.push(Caller::session(self.payer).with_admin_mode(false));
        }
        Ok(Some(callers))
    }
    pub(crate) async fn guard_publication(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Time, EngineError> {
        self.raw.lock_identity(conn).await.map_err(|_| ended())?;
        self.checked(conn, clock, false).await?.ok_or_else(ended)?;
        let current = self.raw.current(conn, clock).await.map_err(|_| ended())?;
        current.check(clock).map_err(|_| ended())?;
        Ok(current
            .until()
            .map_or(self.deadline, |until| until.min(self.deadline)))
    }
    pub(crate) async fn readable(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<(), EngineError> {
        let mut tx = sqlx::Connection::begin(conn).await.map_err(DbError::from)?;
        self.checked(&mut tx, clock, false)
            .await?
            .ok_or_else(ended)?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }
    pub(crate) async fn retained(&self, conn: &mut Conn) -> Result<Option<Json>, EngineError> {
        Ok(sqlx::query_scalar::<_, Option<Json>>(
            "SELECT delegated_program FROM ask_calls WHERE id=$1 AND NOT settled",
        )
        .bind(self.call)
        .fetch_optional(conn)
        .await
        .map_err(DbError::from)?
        .flatten())
    }
    pub(crate) async fn retain(
        &self,
        engine: &crate::Engine,
        record: &Json,
    ) -> Result<Json, EngineError> {
        let mut tx = engine.db().begin().await?;
        self.lock_source(&mut tx).await?;
        self.checked(&mut tx, engine.inner.clock.as_ref(), true)
            .await?
            .ok_or_else(ended)?;
        let saved:Json=sqlx::query_scalar("UPDATE ask_calls SET delegated_program=COALESCE(delegated_program,$2) WHERE id=$1 AND NOT settled RETURNING delegated_program")
            .bind(self.call).bind(record).fetch_one(&mut *tx).await.map_err(DbError::from)?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(saved)
    }
    pub(crate) async fn result(
        &self,
        engine: &crate::Engine,
        result: Json,
    ) -> Result<(), EngineError> {
        let mut tx = engine.db().begin().await?;
        self.lock_source(&mut tx).await?;
        self.checked(&mut tx, engine.inner.clock.as_ref(), false)
            .await?
            .ok_or_else(ended)?;
        let accepted=sqlx::query("UPDATE ask_calls SET delegated_result=COALESCE(delegated_result,$2) WHERE id=$1 AND NOT settled AND (delegated_result IS NULL OR delegated_result=$2)")
            .bind(self.call).bind(result).execute(&mut *tx).await.map_err(DbError::from)?;
        if accepted.rows_affected() != 1 {
            return Err(ended());
        }
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }
}
