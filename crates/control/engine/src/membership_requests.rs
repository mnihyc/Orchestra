//! Group join requests use the shared approval ledger and grant ordinary membership, with an optional deadline.

use crate::approvals::access::{DecideError, Decided, RaiseError, Raised};
use crate::notifications::{self, Event, Kind};
use crate::{Engine, EngineError};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_authz::{Caller, Principal};
use orch_chat::chats::{self, Mode};
use orch_db::audit::{self, Actor, Entry, Origin};
use orch_db::queries::approvals::{self as approvals, ApprovalRow, access};
use orch_db::queries::chats as rows;
use orch_db::{Conn, Tx};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time, new_id};
use serde_json::json;

fn chat_error(error: orch_chat::Error) -> EngineError {
    EngineError::unreadable(error.to_string())
}

/// Whether this access approval grants membership rather than a permission entry.
#[must_use]
pub fn is_join(row: &ApprovalRow) -> bool {
    row.kind == "access" && row.object_kind.as_deref() == Some("chat")
}

async fn outsiders_allowed(conn: &mut Conn, root: Id) -> Result<bool, EngineError> {
    Ok(
        get::<bool>(conn, Place::Node(root), keys::SHARING_ALLOW_EXTERNAL)
            .await?
            .unwrap_or(true),
    )
}

/// The same ordinary add-member rule used by the actual grant. Inspecting a chat does not make one a member.
pub async fn may_add(
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
    requester: Id,
) -> Result<bool, EngineError> {
    if principal.user() == requester {
        return Ok(false);
    }
    let origin = Origin::default();
    let act = orch_chat::Acting {
        principal,
        now: principal.now().get(),
        origin: &origin,
    };
    let loaded = match chats::load(conn, &act, chat, Mode::Read).await {
        Ok(loaded) => loaded,
        Err(orch_chat::Error::Db(error)) => return Err(error.into()),
        Err(_) => return Ok(false),
    };
    let allowed = outsiders_allowed(conn, loaded.row.home()).await?;
    match chats::can_add_member(conn, &act, &loaded, requester, allowed).await {
        Ok(_) => Ok(true),
        Err(orch_chat::Error::Db(error)) => Err(error.into()),
        Err(_) => Ok(false),
    }
}

/// Current humans who may add this particular requester to a known group.
pub async fn deciders(
    conn: &mut Conn,
    chat: Id,
    requester: Id,
    at: Time,
) -> Result<Vec<Id>, EngineError> {
    let Some(row) = rows::chat(conn, chat)
        .await?
        .filter(|row| row.kind == rows::KIND_GROUP)
    else {
        return Ok(Vec::new());
    };
    let members = rows::members(conn, chat).await?;
    let mut candidates: Vec<_> = members
        .iter()
        .filter(|member| member.active_at(at.get()) && member.manager)
        .map(|member| member.user_id)
        .collect();
    candidates.extend(
        orch_authz::approvals::permission_deciders(
            conn,
            ObjectRef {
                kind: ObjectKind::Org,
                id: row.org_id,
            },
            orch_access_contract::permissions::Permission::DirectsView,
            at,
        )
        .await?,
    );
    let graph = orch_authz::engine::Graph::load(conn, &[row.home()]).await?;
    if let Some(home) = graph.site_of(row.home()) {
        candidates.extend(
            orch_authz::approvals::permission_deciders(
                conn,
                ObjectRef {
                    kind: home.kind,
                    id: home.id,
                },
                orch_access_contract::permissions::Permission::ChatManage,
                at,
            )
            .await?,
        );
    }
    candidates.sort_unstable();
    candidates.dedup();
    let mut people = Vec::new();
    for person in candidates.into_iter().filter(|person| *person != requester) {
        let Ok(principal) =
            Principal::load(conn, Caller::session(person).with_admin_mode(true), at).await
        else {
            continue;
        };
        if may_add(conn, &principal, row.id, requester).await? {
            people.push(person);
        }
    }
    people.sort_unstable();
    people.dedup();
    Ok(people)
}

/// True only for a live membership. Inspection and shared agent reach do not count as joining.
pub async fn is_member(
    conn: &mut Conn,
    chat: Id,
    person: Id,
    at: Time,
) -> Result<bool, EngineError> {
    Ok(rows::members(conn, chat)
        .await?
        .iter()
        .any(|member| member.user_id == person && member.active_at(at.get())))
}

/// Only the existing native group request's materialized facts, never serialized or accepted as credentials.
pub struct PreparedJoinRequest {
    requester: Id,
    now: Time,
    chat: Id,
    org: Id,
    eligible: Vec<Id>,
    people: Vec<Id>,
    end: chrono::DateTime<chrono::Utc>,
    batch: Id,
    approval: Id,
    request: serde_json::Value,
    title: &'static str,
    reason: String,
    requested_expires_at: Option<Time>,
}

impl Engine {
    /// Raises a membership request in the caller's guarded transaction, with the ordinary access-request limits.
    #[expect(
        clippy::too_many_arguments,
        reason = "the guarded transaction and the complete membership request commit together"
    )]
    pub async fn ask_to_join_guarded(
        &self,
        mut tx: Tx,
        principal: Principal,
        chat: Id,
        reason: &str,
        label: Option<&str>,
        requested_expires_at: Option<Time>,
        origin: &Origin,
    ) -> Result<Raised, RaiseError> {
        self.prepare_join_request_prefix(&mut tx, principal.user(), chat)
            .await?;
        let prepared = self
            .prepare_join_request(
                &mut tx,
                &principal,
                chat,
                reason,
                label,
                requested_expires_at,
            )
            .await?;
        self.raise_prepared_join_request(tx, prepared, origin).await
    }

    /// Existing requester serialization and old-card expiry are staged maintenance before
    /// accepting a new ask. HTTP rechecks its complete Guard after these actual waits.
    pub async fn prepare_join_request_prefix(
        &self,
        conn: &mut Conn,
        requester: Id,
        chat: Id,
    ) -> Result<(), RaiseError> {
        access::lock_requester(conn, requester).await?;
        let now = self.inner.clock.time();
        let row = rows::chat(conn, chat)
            .await?
            .filter(|row| row.kind == rows::KIND_GROUP)
            .ok_or(RaiseError::Item {
                index: 0,
                error: orch_authz::requests::AskError::NotFound,
            })?;
        if is_member(conn, row.id, requester, now).await? {
            return Err(RaiseError::Item {
                index: 0,
                error: orch_authz::requests::AskError::Held,
            });
        }
        for id in access::expired_join_requests(conn, chat, requester, now.get()).await? {
            if access::expire(conn, id, now.get()).await? {
                orch_db::queries::notifications::inbox::settle_approval(conn, id, now.get())
                    .await?;
            }
        }
        Ok(())
    }

    /// Current native group/giver/body preparation; no new request is inserted here.
    pub async fn prepare_join_request(
        &self,
        conn: &mut Conn,
        principal: &Principal,
        chat: Id,
        reason: &str,
        label: Option<&str>,
        requested_expires_at: Option<Time>,
    ) -> Result<PreparedJoinRequest, RaiseError> {
        let requested_expires_at = requested_expires_at.map(Time::to_minute);
        let current =
            Principal::load(conn, principal.caller().clone(), self.inner.clock.time()).await?;
        let current = match principal.second_step_hold() {
            Some(hold) => current.held_for_second_step(hold.clone()),
            None => current,
        };
        let principal = &current;
        let now = principal.now();
        if requested_expires_at.is_some_and(|end| end <= now) {
            return Err(RaiseError::Item {
                index: 0,
                error: orch_authz::requests::AskError::Invalid {
                    path: "expires_at",
                    message: "must be in a future minute".to_owned(),
                },
            });
        }
        let row = rows::chat(&mut *conn, chat)
            .await?
            .filter(|row| row.kind == rows::KIND_GROUP)
            .ok_or(RaiseError::Item {
                index: 0,
                error: orch_authz::requests::AskError::NotFound,
            })?;
        if is_member(&mut *conn, chat, principal.user(), now).await? {
            return Err(RaiseError::Item {
                index: 0,
                error: orch_authz::requests::AskError::Held,
            });
        }
        if !access::expired_join_requests(conn, chat, principal.user(), now.get())
            .await?
            .is_empty()
        {
            return Err(orch_db::DbError::Retry.into());
        }
        if let Some(waiting) =
            access::pending_for_person(&mut *conn, principal.user(), chat, now.get()).await?
        {
            return Err(RaiseError::AlreadyWaiting {
                approval: waiting.id,
            });
        }
        let most = get::<i64>(&mut *conn, Place::System, keys::ACCESS_MAX_PENDING_REQUESTS)
            .await
            .map_err(EngineError::from)?
            .unwrap_or(crate::approvals::access::MOST_WAITING);
        let waiting = access::waiting_batches(&mut *conn, principal.user(), now.get()).await?;
        if waiting >= most {
            return Err(RaiseError::Waiting {
                limit: most,
                waiting,
            });
        }
        let eligible = deciders(&mut *conn, chat, principal.user(), now).await?;
        if eligible.is_empty() {
            return Err(RaiseError::Item {
                index: 0,
                error: orch_authz::requests::AskError::Nobody,
            });
        }
        let mut people: Vec<_> = rows::members(conn, chat)
            .await?
            .into_iter()
            .filter(|member| {
                member.manager && member.active_at(now.get()) && eligible.contains(&member.user_id)
            })
            .map(|member| member.user_id)
            .collect();
        people.sort_unstable();
        people.dedup();
        if people.is_empty() {
            let graph = orch_authz::engine::Graph::load(conn, &[row.home()]).await?;
            let home = graph
                .site_of(row.home())
                .ok_or_else(|| EngineError::unreadable("the group home is unavailable"))?;
            people = orch_authz::approvals::owners_to_tell_beside(
                conn,
                orch_db::queries::access::PageKey::new("chat", chat, "live"),
                ObjectRef {
                    kind: home.kind,
                    id: home.id,
                },
                &eligible,
                now,
            )
            .await?;
        }
        let minutes = get::<i64>(
            &mut *conn,
            Place::System,
            keys::ACCESS_REQUEST_EXPIRY_MINUTES,
        )
        .await
        .map_err(EngineError::from)?
        .unwrap_or(crate::approvals::access::EXPIRY_MINUTES);
        let end = Time::new(
            chrono::Duration::try_minutes(minutes)
                .and_then(|span| now.get().checked_add_signed(span))
                .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC),
        )
        .to_minute()
        .get();
        let batch = new_id();
        let request = json!({"membership":true,"label":label,"permissions":[],"holder":{"kind":"user","user":crate::views::user_ref_json(&mut *conn,principal.user()).await?},"reason":reason,"for_thread":null});
        let title = "A person asks to join a group chat";
        Ok(PreparedJoinRequest {
            requester: principal.user(),
            now,
            chat,
            org: row.org_id,
            eligible,
            people,
            end,
            batch,
            approval: new_id(),
            request,
            title,
            reason: reason.to_owned(),
            requested_expires_at,
        })
    }

    /// Consumes the same native facts; its first await inserts the actual new approval.
    pub async fn raise_prepared_join_request(
        &self,
        mut tx: Tx,
        prepared: PreparedJoinRequest,
        origin: &Origin,
    ) -> Result<Raised, RaiseError> {
        let PreparedJoinRequest {
            requester,
            now,
            chat,
            org,
            eligible,
            people,
            end,
            batch,
            approval: id,
            request,
            title,
            reason,
            requested_expires_at,
        } = prepared;
        let approval = access::insert(
            &mut tx,
            &access::NewAccessApproval {
                id,
                batch_id: batch,
                node_id: Some(org),
                object_kind: Some("chat"),
                object_id: Some(chat),
                thread_id: None,
                requester,
                title,
                request: &request,
                required_permission: orch_access_contract::permissions::Permission::ChatInteract
                    .as_str(),
                expires_at: end,
                requested_expires_at: requested_expires_at.map(Time::get),
                at: now.get(),
            },
        )
        .await?;
        let mut audit = Entry::new(
            Actor::User(requester),
            orch_db::audit::action!(AuditAction::AccessRequested).as_str(),
            "access",
            Some(approval.id),
            "asked to join a group chat",
        );
        audit.org_id = Some(org);
        audit.details = json!({"chat_id":chat,"approval_id":approval.id,"batch_id":batch,"reason":reason,"requested_expires_at":requested_expires_at});
        audit::append(&mut tx, &audit, origin, now.get()).await?;
        let event = Event {
            kind: Kind::AccessRequested,
            node: None,
            thread: None,
            approval: Some(approval.id),
            title: title.to_owned(),
            body: reason.to_owned(),
            data: json!({"approval_id":approval.id,"batch_id":batch,"requested_expires_at":requested_expires_at,"path":"/approvals"}),
            source: approval.id.to_string(),
            actor: Some(requester),
            at: now.get(),
        };
        notifications::tell(&mut tx, &event, &people, Some(org)).await?;
        orch_db::idempotency::creation::observe_with(
            &mut tx,
            Some(requester),
            "chat_join_request",
            batch,
            json!({"chat":chat,"approvals":[approval.id]}),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.inner.live.approvals_changed(&eligible);
        Ok(Raised {
            batch_id: batch,
            approvals: vec![approval],
        })
    }

    /// Settles the shared request and the corresponding membership change in one transaction.
    pub(crate) async fn decide_join_guarded(
        &self,
        mut tx: Tx,
        principal: Principal,
        row: ApprovalRow,
        decided: &Decided,
        origin: &Origin,
    ) -> Result<ApprovalRow, DecideError> {
        let chat = row.object_id.ok_or(DecideError::NotFound)?;
        let requester = row.requester_user_id.ok_or(DecideError::NotFound)?;
        if principal.user() == requester {
            return Err(DecideError::OwnRequest);
        }
        let now = principal.now();
        let act = orch_chat::Acting {
            principal: &principal,
            now: now.get(),
            origin,
        };
        // Chat first, approval second, as for direct membership changes and expiry.
        let mut loaded = chats::load(&mut tx, &act, chat, Mode::Write)
            .await
            .map_err(|_| DecideError::NotFound)?;
        let eligible = deciders(&mut tx, chat, requester, now).await?;
        if !may_add(&mut tx, &principal, chat, requester).await? {
            return Err(DecideError::Invalid {
                path: "",
                message: "you may no longer add a member to this group".to_owned(),
            });
        }
        if decided
            .permissions
            .as_ref()
            .is_some_and(|permissions| !permissions.is_empty())
            || decided.options.is_some()
        {
            return Err(DecideError::Invalid {
                path: "permissions",
                message: "joining gives membership, with an optional end date".to_owned(),
            });
        }
        if decided.expires_at.is_some_and(|end| end <= now) {
            return Err(DecideError::Invalid {
                path: "expires_at",
                message: "must be in the future".to_owned(),
            });
        }
        let state = if decided.approve {
            "approved"
        } else {
            "denied"
        };
        let decision = json!({"approve":decided.approve,"permissions":[],"expires_at":decided.expires_at,"note":decided.note,"membership":true});
        if !approvals::decide(
            &mut tx,
            row.id,
            state,
            Some(principal.user()),
            &decision,
            now.get(),
        )
        .await?
        {
            return Err(DecideError::AlreadyDecided);
        }
        if decided.approve {
            let allowed = outsiders_allowed(&mut tx, loaded.row.org_id).await?;
            chats::add_member_until(
                &mut tx,
                &act,
                &mut loaded,
                requester,
                allowed,
                decided.expires_at.map(Time::get),
            )
            .await
            .map_err(|error| DecideError::Invalid {
                path: "",
                message: error.to_string(),
            })?;
        }
        let mut entry = Entry::new(
            Actor::User(principal.user()),
            if decided.approve {
                orch_db::audit::action!(AuditAction::AccessApproved).as_str()
            } else {
                orch_db::audit::action!(AuditAction::AccessDenied).as_str()
            },
            "access",
            Some(row.id),
            if decided.approve {
                "approved a request to join a group chat"
            } else {
                "denied a request to join a group chat"
            },
        );
        entry.org_id = Some(loaded.row.org_id);
        entry.details = json!({"approval_id":row.id,"chat_id":chat,"requester":requester,"requested_expires_at":row.requested_expires_at.map(Time::new),"expires_at":decided.expires_at,"note":decided.note});
        audit::append(&mut tx, &entry, origin, now.get()).await?;
        let event = Event {
            kind: Kind::AccessDecided,
            node: None,
            thread: None,
            approval: Some(row.id),
            title: if decided.approve {
                "Your request to join was approved"
            } else {
                "Your request to join was denied"
            }
            .to_owned(),
            body: decided.note.clone().unwrap_or_default(),
            data: json!({"approval_id":row.id,"batch_id":row.batch_id,"approved":decided.approve,"membership":true,"requested_expires_at":row.requested_expires_at.map(Time::new),"path":"/approvals"}),
            source: format!("{}:{state}", row.id),
            actor: Some(principal.user()),
            at: now.get(),
        };
        notifications::tell(&mut tx, &event, &[requester], Some(loaded.row.org_id)).await?;
        let settled = approvals::approval(&mut tx, row.id)
            .await?
            .ok_or(DecideError::NotFound)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let mut readers = eligible;
        readers.push(requester);
        self.inner.live.approvals_changed(&readers);
        Ok(settled)
    }

    /// Makes timed removals durable and publishes the ordinary membership changes.
    pub async fn expire_chat_memberships(&self) -> Result<(), EngineError> {
        let mut tx = self.inner.db.begin().await?;
        chats::expire_memberships(
            &mut tx,
            self.inner.clock.now(),
            self.inner.config.sweep_batch,
        )
        .await
        .map_err(chat_error)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }
}
