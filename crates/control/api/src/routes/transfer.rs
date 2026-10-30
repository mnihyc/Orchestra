//! Handing a resource or a document over to another person, who is its first owner from then on.
//!
//! One of the owners or an admin of the place the thing is in names the person in the Owners row and
//! says that they are first; the person who was first stays an owner unless the same request takes
//! them out. It is the engine's change of the Owners row, in one transaction, audited as the passing
//! on of the kind of thing it is, and the answer is the thing as its own request reads it. A node is
//! handed over by its own request; a kind that has no page here is answered plainly as not there.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::access::subject::{self, Named};
use crate::routes::nodes::invalid;
use crate::routes::{documents, environments, resources};
use crate::sign_in::origin;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::ownership::{ObjectTransfer, TransferBody};
use orch_api_contract::resources::Resource;
use orch_authz::pages::ChangeError;
use orch_authz::things::Thing;
use orch_authz::transfer::{TransferError, hand_over};
use orch_db::DbError;
use orch_db::queries::documents as documents_rows;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(ObjectTransfer, transfer);
}

/// The kinds a transfer answers for: those whose own request reads the thing back.
fn require_transfer_kind(kind: ObjectKind) -> ApiResult<()> {
    match kind {
        ObjectKind::Provider
        | ObjectKind::Agent
        | ObjectKind::Environment
        | ObjectKind::Host
        | ObjectKind::Machine
        | ObjectKind::Template
        | ObjectKind::Mount
        | ObjectKind::McpServer
        | ObjectKind::Skill
        | ObjectKind::Document
        | ObjectKind::DocumentContainer => Ok(()),
        _ => Err(ApiError::new(
            ErrorCode::NotFound,
            "Ownership can be transferred here for providers, agent profiles, environments, hosts, machines, templates, mounts, tool servers, skills, and documents.",
        )),
    }
}

/// The thing as the request that reads it shows it to the caller as they stand now: what a caller
/// who holds nothing of it may see is what that request shows to anybody who holds some part of it
/// and no more, so that whoever handed a thing over, or administers it, is answered all the same.
async fn as_read(ctx: &Ctx, object: ObjectRef) -> ApiResult<Resource> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    let id = object.id;
    Ok(match object.kind {
        ObjectKind::Provider => {
            Resource::Provider(resources::providers::show(ctx, &mut conn, &principal, id).await?)
        }
        ObjectKind::Agent => {
            Resource::Agent(resources::agents::show(ctx, &mut conn, &principal, id).await?)
        }
        ObjectKind::Environment => {
            Resource::Environment(environments::show(ctx, &mut conn, &principal, id).await?)
        }
        ObjectKind::Host => {
            Resource::Host(resources::hosts::show(ctx, &mut conn, &principal, id).await?)
        }
        ObjectKind::Machine => {
            let row = orch_db::queries::resources::machine(&mut conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Resource::Machine(resources::machines::machine_view(ctx, &mut conn, row).await?)
        }
        ObjectKind::Template => {
            Resource::Template(resources::templates::show(&mut conn, &principal, id).await?)
        }
        ObjectKind::Mount => {
            Resource::Mount(resources::mounts::show(ctx, &mut conn, &principal, id).await?)
        }
        ObjectKind::McpServer => {
            Resource::McpServer(resources::mcp::show(ctx, &mut conn, &principal, id).await?)
        }
        ObjectKind::Skill => {
            Resource::Skill(resources::skills::show(&mut conn, &principal, id).await?)
        }
        ObjectKind::Document => {
            drop(conn);
            Resource::Document(
                documents::get(
                    ctx.clone(),
                    orch_api_contract::nodes::DeletedInspectionQuery::default(),
                    (),
                )
                .await?,
            )
        }
        ObjectKind::DocumentContainer => {
            let read = orch_engine::document_tree::read_in(
                &mut conn,
                orch_engine::document_tree::Actor::Person(&principal),
                orch_engine::document_tree::Item::Container(id),
                orch_types::Time::new(ctx.now),
            )
            .await;
            let read = crate::routes::document_tree::result(&mut conn, read).await?;
            Resource::DocumentContainer(crate::routes::document_tree::convert(read)?)
        }
        _ => return Err(ApiError::not_found()),
    })
}

/// Makes another person the first owner of a resource or a document.
async fn transfer(ctx: Ctx, (): (), body: TransferBody) -> ApiResult<Resource> {
    let Named { thing } = subject::named(&ctx)?;
    let Thing::Other(object) = thing else {
        // A node is handed over by `POST /nodes/{id}/transfer`.
        return Err(ApiError::new(
            ErrorCode::NotFound,
            "a node is handed over by its own request",
        ));
    };
    require_transfer_kind(object.kind)?;
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let (_, site) =
        orch_authz::things::site(&mut tx, thing)
            .await
            .map_err(|error| match error {
                orch_authz::Error::NotFound(_) => ApiError::not_found(),
                other => ApiError::internal(other),
            })?;
    let policies = subject::resource_policy(&mut tx, thing, &site).await?;
    let handed = hand_over(
        &mut tx,
        &principal,
        thing,
        body.user_id,
        body.keep_previous.unwrap_or(true),
        &policies,
        &origin(&ctx),
    )
    .await;
    match handed {
        Ok(handed) => {
            // Whoever has the document open learns that its page changed.
            if handed.changed && object.kind == ObjectKind::Document {
                documents_rows::touch(&mut tx, object.id).await?;
            }
        }
        Err(TransferError::Engine(error)) => return Err(refusals::problem(&mut tx, error).await),
        Err(TransferError::NoSuchUser) => return Err(invalid("user_id", "is no user")),
        Err(TransferError::NotAnActiveMember) => {
            return Err(invalid(
                "user_id",
                "is not an active member of the org this lies in",
            ));
        }
        Err(TransferError::Change(error)) => {
            return Err(match *error {
                ChangeError::FirstOwnerStays { owner } => ApiError::new(
                    ErrorCode::ResourceInUse,
                    "this is registered from a person's machine, and keeps that machine's owner as its first owner",
                )
                .with("by", "machine")
                .with("owner", owner),
                other => refusals::change_failed(&mut tx, other).await,
            });
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    as_read(&ctx, object).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_kind_refusals_name_the_kinds_the_request_answers_for() {
        for kind in [
            ObjectKind::Provider,
            ObjectKind::Agent,
            ObjectKind::Environment,
            ObjectKind::Host,
            ObjectKind::Machine,
            ObjectKind::Template,
            ObjectKind::Mount,
            ObjectKind::McpServer,
            ObjectKind::Skill,
            ObjectKind::Document,
            ObjectKind::DocumentContainer,
        ] {
            assert!(require_transfer_kind(kind).is_ok(), "{kind:?}");
        }
        // Place transfers have their own request; this resource guard carries
        // a factual answer even if a future caller reaches it directly.
        for kind in [
            ObjectKind::Org,
            ObjectKind::Project,
            ObjectKind::Topic,
            ObjectKind::Thread,
        ] {
            let Err(error) = require_transfer_kind(kind) else {
                panic!("{kind:?} has its own transfer request");
            };
            assert_eq!(error.code(), ErrorCode::NotFound);
            assert_eq!(
                error.problem.detail,
                "Ownership can be transferred here for providers, agent profiles, environments, hosts, machines, templates, mounts, tool servers, skills, and documents."
            );
        }
    }
}
