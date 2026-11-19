//! Display clients use the real shipped-image peer and the same guarded API fixture as browsers.
//! This is a shared test module, matching browsers_support rather than a standalone integration crate.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(
    dead_code,
    reason = "each display case uses part of the shared protocol client"
)]

use super::browsers_support::{Floor, WAIT};
use crate::places::Who;
use orch_api_contract::{browsers::*, displays::*};
use orch_link_contract::commands::{Command, CommandKind, OrchToolResult};
use orch_types::Id;

mod screen;
#[allow(
    unused_imports,
    reason = "each test binary uses the part of the screen support it needs"
)]
pub use screen::{Screen, key, paste};

impl Screen {
    /// Connects a viewer without receiving or supplying any VNC password.
    pub async fn attach(floor: &Floor, by: &Who, display: Id, control: bool) -> Self {
        let ticket = floor
            .scene
            .rig
            .world
            .app
            .call(DisplayTicketCreate, &[&display])
            .token(&by.token)
            .body(&DisplayTicketCreateBody {
                control: Some(control),
            })
            .send()
            .await
            .ok();
        Self::with_ticket(floor, display, &ticket.ticket).await
    }
    /// Completes the real viewer handshake with one already issued ticket.
    pub async fn with_ticket(floor: &Floor, display: Id, ticket: &str) -> Self {
        let address = floor.serve().await;
        Self::connect_at(
            address,
            &format!("/api/v1/ws/displays/{display}?ticket={ticket}"),
        )
        .await
    }
}

/// Opens a real Chromium window on the same Xvnc implementation displays use.
pub async fn window(floor: &Floor) -> Browser {
    floor
        .scene
        .rig
        .world
        .app
        .call(EnvironmentBrowserCreate, &[&floor.environment])
        .token(&floor.scene.ada.token)
        .body(&EnvironmentBrowserCreateBody {
            profile: "display-program".to_owned(),
            create: Some(true),
            preset: Some(BrowserPreset::Window),
            url: Some("http://127.0.0.1:41820/".to_owned()),
        })
        .send()
        .await
        .ok()
}
/// Dispatches the actual display tool, rather than calling its lifecycle helper directly.
pub async fn call(floor: &Floor, thread: Id, arguments: orch_types::Json) -> OrchToolResult {
    let call_ref = orch_types::new_id().to_string();
    floor
        .app
        .engine
        .answer_tool_call(&orch_engine::tools::CallContext {
            machine: floor.scene.rig.machine_id,
            thread,
            turn: orch_types::new_id(),
            call_ref: call_ref.clone(),
            attempt: None,
            tool: orch_agent_contract::events::OrchTool::UseDisplay,
            arguments,
        })
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(result) = floor
                .scene
                .rig
                .seen
                .of_kind(CommandKind::OrchToolResult)
                .into_iter()
                .find_map(|command| match command {
                    Command::OrchToolResult(result)
                        if result.thread_id == thread && result.call_ref == call_ref =>
                    {
                        Some(result)
                    }
                    _ => None,
                })
            {
                return result;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

/// A test command sink opens only its known fixture key before passing the private value to Xvnc.
pub fn password_commands(floor: &Floor) {
    let peer = std::sync::Arc::clone(&floor.peer);
    let db = floor.scene.rig.db.clone();
    floor
        .scene
        .rig
        .machine
        .handler
        .on(CommandKind::DisplayAdopt, move |command| {
            let password = match &command {
                Command::DisplayAdopt(body) => match body.password {
                    Some(orch_link_contract::commands::Secret::Ref(id)) => {
                        Some(tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                let mut conn = db.acquire().await.unwrap();
                                let row = orch_db::queries::displays::get(&mut conn, id)
                                    .await
                                    .unwrap()
                                    .unwrap();
                                String::from_utf8(
                                    orch_db::secrets::MasterKey::new([7; 32])
                                        .open(
                                            row.password.as_ref().unwrap(),
                                            format!("display/{id}/password").as_bytes(),
                                        )
                                        .unwrap(),
                                )
                                .unwrap()
                            })
                        }))
                    }
                    None => None,
                    _ => panic!("the test sink needs a stored secret reference"),
                },
                _ => panic!("the test sink only handles display.adopt"),
            };
            let result = peer.rpc(serde_json::json!({"command":command,"password":password}));
            if let Some(error) = result.get("error") {
                return Err(orch_link_contract::commands::CommandError {
                    code: error["code"].as_str().unwrap().to_owned(),
                    detail: error["detail"].as_str().unwrap().to_owned(),
                    retryable: false,
                    details: None,
                });
            }
            Ok(
                serde_json::from_value::<orch_link_contract::commands::CommandResult>(
                    result["ok"].clone(),
                )
                .unwrap(),
            )
        });
}
