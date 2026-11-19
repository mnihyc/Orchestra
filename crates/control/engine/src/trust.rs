//! A workspace's own harness configuration: the question the people who manage an environment answer once, what a
//! thread does while it waits, and the answer changed on the environment's page.
//!
//! An adapter that finds a configuration of its harness's own in the workspace (a `.codex` or `.claude` with
//! anything in it but empty directories) on the way to where a session works refuses the session with
//! `workspace_configuration` while the environment has no answer for the files it finds. The turn that asked is
//! given up without an error of the thread's own: its input waits again where it stood, and the thread waits
//! (`workspace_trust`) for the question, which the environment's `config_trust` holds, asked once for the
//! environment and the harness however many threads meet it, and asked again when the files change. Those who
//! hold Manage on the environment answer it, a person and never an agent, in the thread or on the environment's
//! page: Trust, and the harness uses the configuration as in a folder it trusts; Keep it out, and it works
//! without it as in a folder it does not. Every session opened after the answer carries it, and the threads that
//! waited go on. A directory the harness made for itself and left empty is no configuration, and nobody is asked
//! about it.

use crate::approvals::ApprovalError;
use crate::engine::Engine;
use crate::error::EngineError;
use crate::preflight::Blocked;
use crate::sessions::StartSnapshot;
use crate::unit::Unit;
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::{Decision, ObjectRef as AccessObject, Refusal};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_agent_contract::adapter::AdapterError;
use orch_api_contract::trust::ConfigTrustAnswer;
use orch_authz::{Principal, resources};
use orch_db::Conn;
use orch_db::audit::{Actor, Entry, Origin, append};
use orch_db::queries::environments::thread_engine::EnvironmentRow;
use orch_db::queries::trust::{self as rows, answer};
use orch_link_contract::commands as link;
use orch_types::Id;
use serde_json::json;

/// Why a thread waits while the question is open, as its blocked reason.
pub const WAITING: &str = rows::WAITING_REASON;

/// The code of an adapter's refusal of a workspace that holds a configuration nobody has answered for.
pub const REFUSAL: &str = orch_agent_contract::session::WORKSPACE_CONFIGURATION;

/// The word an answer is kept in.
#[must_use]
pub const fn answer_word(answer: ConfigTrustAnswer) -> &'static str {
    match answer {
        ConfigTrustAnswer::Trust => answer::TRUST,
        ConfigTrustAnswer::KeepOut => answer::KEEP_OUT,
    }
}

/// What a session of `harness_kind` in `environment` is opened with: the answer and the files it holds for, where
/// the environment has one.
pub(crate) async fn answer_for(
    conn: &mut Conn,
    environment: Id,
    harness_kind: &str,
) -> Result<Option<link::WorkspaceConfiguration>, EngineError> {
    let Some(member) = rows::of_harness(conn, environment, harness_kind).await? else {
        return Ok(None);
    };
    Ok(match member.answer.as_deref() {
        Some(answer::TRUST) => Some(link::WorkspaceConfiguration {
            answer: link::ConfigTrust::Trust,
            files: member.files,
        }),
        Some(answer::KEEP_OUT) => Some(link::WorkspaceConfiguration {
            answer: link::ConfigTrust::KeepOut,
            files: member.files,
        }),
        _ => None,
    })
}

/// Whether a thread that would start a turn of `harness_kind` in `environment` waits for the question about its
/// workspace's own configuration: it does while the environment's member for that harness has no answer.
pub(crate) async fn waiting(
    conn: &mut Conn,
    environment: &EnvironmentRow,
    harness_kind: &str,
) -> Result<Option<Blocked>, EngineError> {
    let member = rows::of_harness(conn, environment.id, harness_kind).await?;
    Ok(member.filter(|member| member.answer.is_none()).map(|member| Blocked {
        reason: WAITING,
        detail: Some(format!(
            "the people who manage {} decide whether the harness may use the configuration its workspace holds at {}",
            environment.name,
            member.path()
        )),
    }))
}

/// Asks the people who manage the environment of a turn whose session its harness refused for the workspace's own
/// configuration, once for the environment, the harness and the files found: `detail` is the refusal's, which
/// names them. The answer says whether the thread waits for the question; it does not where the environment was
/// answered for these files meanwhile, and the turn is simply tried again.
pub(crate) async fn ask(
    unit: &mut Unit<'_>,
    snap: &StartSnapshot,
    detail: &str,
) -> Result<bool, EngineError> {
    let environment = snap.environment.id;
    let harness_kind = snap.agent.harness_kind.clone();
    let files = AdapterError::configuration_files(detail);
    let found = AdapterError::configuration_found(detail);
    if let Some(member) = rows::of_harness(&mut *unit.conn, environment, &harness_kind).await?
        && member.answer.is_some()
        && member.files == files
    {
        return Ok(false);
    }
    let now = unit.now();
    let asked = rows::ask(
        &mut *unit.conn,
        environment,
        &harness_kind,
        &files,
        &found,
        now,
    )
    .await?;
    if asked {
        // Its first owner is told at once in their Inbox; whoever holds Manage on it answers in the thread or on
        // the environment's page.
        let harness = snap.descriptor.harness.name.clone();
        let event = crate::notifications::Event {
            kind: crate::notifications::Kind::ThreadBlocked,
            node: Some(snap.thread.node_id),
            thread: Some(snap.thread.node_id),
            approval: None,
            title: format!("Trust this workspace's {harness} configuration?"),
            body: format!(
                "{harness} found a configuration of its own in the workspace of {}: {}",
                snap.environment.name,
                found.join(", ")
            ),
            data: json!({ "reason": WAITING, "environment_id": environment, "harness": harness_kind }),
            source: format!("{environment}:{harness_kind}:{files}"),
            actor: None,
            at: now,
        };
        crate::notifications::tell(
            &mut *unit.conn,
            &event,
            &[snap.environment.owner_id],
            Some(snap.environment.org_id),
        )
        .await?;
    }
    Ok(true)
}

impl Engine {
    /// Whether a caller may answer for the workspace's own configuration of an environment: a person, never an
    /// agent or a service, who holds Manage on the environment.
    async fn may_answer(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        environment: Id,
    ) -> Result<Result<(), Box<Refusal>>, EngineError> {
        let object = AccessObject {
            kind: ObjectKind::Environment,
            id: environment,
        };
        let refused = || {
            Box::new(Refusal::of(
                Permission::EnvironmentManage,
                object,
                Vec::new(),
            ))
        };
        let user = principal.caller().user();
        if orch_db::queries::notifications::subscribers::people_among(&mut *conn, &[user])
            .await?
            .is_empty()
        {
            return Ok(Err(refused()));
        }
        let manages = matches!(
            resources::check(
                conn,
                principal,
                object,
                Permission::EnvironmentManage,
                orch_authz::Policy::default(),
            )
            .await?,
            Decision::Allowed { .. }
        );
        Ok(if manages { Ok(()) } else { Err(refused()) })
    }

    /// Answers, or changes the answer, for one harness whose configuration the environment's workspace holds, in
    /// the transaction the API's action guard asked again in: the environment remembers it for the files the
    /// question was asked about, the act is in the audit, the threads that waited go on, and the sessions already
    /// open with another answer are opened anew at their next turn. A harness nobody was asked about is not found.
    pub async fn set_config_trust_guarded(
        &self,
        mut tx: orch_db::Tx,
        principal: Principal,
        environment: Id,
        harness_kind: &str,
        answer: ConfigTrustAnswer,
    ) -> Result<(), ApprovalError> {
        let now = principal.now().get();
        let by = principal.caller().user();
        let word = answer_word(answer);
        let (finished, reopen) = {
            let mut unit = Unit::new(&mut tx, now);
            if orch_db::queries::environments::thread_engine::environment(
                &mut *unit.conn,
                environment,
            )
            .await?
            .filter(|row| row.deleted_at.is_none())
            .is_none()
            {
                return Err(ApprovalError::NotFound);
            }
            if let Err(refusal) = self
                .may_answer(&mut *unit.conn, &principal, environment)
                .await?
            {
                return Err(ApprovalError::Denied(refusal));
            }
            let Some(before) =
                rows::answer(&mut *unit.conn, environment, harness_kind, word, by, now).await?
            else {
                return Err(ApprovalError::NotFound);
            };
            let mut entry = Entry::new(
                Actor::User(by),
                AuditAction::ApprovalDecided.as_str(),
                "environment",
                Some(environment),
                "Whether a harness may use the configuration its workspace holds was answered.",
            );
            entry.environment_id = Some(environment);
            entry.details = json!({
                "harness": harness_kind,
                "answer": word,
                "before": before.answer,
                "found": before.found,
            });
            append(&mut *unit.conn, &entry, &Origin::default(), now).await?;
            for thread in rows::threads_waiting(&mut *unit.conn, environment).await? {
                unit.wake(thread);
            }
            // A changed answer reaches the sessions already open by their being opened anew at the next turn.
            let changed = before
                .answer
                .as_deref()
                .is_some_and(|before| before != word);
            let reopen = if changed {
                orch_db::queries::threads::tools::threads_in_environment(
                    &mut *unit.conn,
                    environment,
                )
                .await?
            } else {
                Vec::new()
            };
            (unit.finish().await?, reopen)
        };
        self.commit(tx, finished).await?;
        if !reopen.is_empty() {
            let mut open = crate::engine::lock(&self.inner.sessions);
            for thread in &reopen {
                open.forget_thread_sessions(*thread);
            }
        }
        Ok(())
    }
}
