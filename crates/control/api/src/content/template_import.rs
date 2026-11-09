//! The existing native content listener keeps a proposal's original archive source.

use crate::key_operations::native::blob_delivery::{self, Callers, Source};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::state::App;
use futures::future::BoxFuture;
use orch_db::{Conn, queries::link};
use orch_engine::{commands::CommandAuthority, template_imports::Input};
use orch_link_contract::commands::{CommandKind, NodeTemplateImport};
use orch_types::{Hash, Id};

pub(super) struct Archives;
pub(super) enum Opened {
    Original(Box<Input>),
    Legacy,
}
fn refused() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::AuthzDenied,
        "the original native archive source is unavailable",
    )
}
impl Source for Archives {
    type Input = ();
    type Opened = Opened;
    fn prepare<'a>(
        app: &'a App,
        conn: &'a mut Conn,
        command: &'a link::CommandRow,
        authority: Option<&'a CommandAuthority>,
        hash: &'a Hash,
        _input: &'a (),
    ) -> BoxFuture<'a, ApiResult<Option<(Opened, Callers)>>> {
        Box::pin(async move {
            if command.kind != CommandKind::NodeTemplateImport.as_str() {
                return Ok(None);
            }
            let required = link::command_authority_required(conn, command.id).await?;
            match authority {
                Some(CommandAuthority::TemplateImport(source)) if required => Ok(source
                    .current_bytes(conn, command, hash, app.clock.as_ref())
                    .await
                    .map_err(ApiError::engine)?
                    .map(|callers| (Opened::Original(Box::new(source.input().clone())), callers))),
                None if !required => {
                    // Genuine pre-existing/system native work has no invented HTTP identity.
                    let body: NodeTemplateImport =
                        serde_json::from_value(command.body.clone()).map_err(|_| refused())?;
                    let archives = orch_driver_contract::imports::archive_references(&body.values)
                        .map_err(|_| refused())?;
                    Ok(archives
                        .iter()
                        .any(|archive| &archive.hash == hash)
                        .then_some((Opened::Legacy, Vec::new())))
                }
                _ => Ok(None),
            }
        })
    }
    fn refresh<'a>(
        app: &'a App,
        conn: &'a mut Conn,
        command: &'a link::CommandRow,
        authority: Option<&'a CommandAuthority>,
        hash: &'a Hash,
        input: &'a (),
        opened: &'a Opened,
    ) -> BoxFuture<'a, ApiResult<Option<Callers>>> {
        Box::pin(async move {
            let Some((fresh, callers)) =
                Self::prepare(app, conn, command, authority, hash, input).await?
            else {
                return Ok(None);
            };
            Ok(match (opened, fresh) {
                (Opened::Original(before), Opened::Original(now)) if before == &now => {
                    Some(callers)
                }
                (Opened::Legacy, Opened::Legacy) => Some(callers),
                _ => None,
            })
        })
    }
}
pub(super) async fn prepare(
    app: &App,
    machine: Id,
    command: Id,
    hash: Hash,
) -> ApiResult<blob_delivery::Delivery<Archives>> {
    blob_delivery::prepare::<Archives>(app, machine, command, hash, ()).await
}
