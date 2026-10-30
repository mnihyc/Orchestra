//! Canonical web addresses for already authorized names and inventory rows.
//!
//! Callers supply the thing's own place and org, never the page from which somebody searched.

use orch_api_contract::nodes::{NodeKind, NodeRef};
use orch_types::Id;

/// A named thing's screen, or no address where choosing a group/project is still necessary.
pub(super) fn href(
    kind: &str,
    id: &str,
    node: Option<Id>,
    org: Option<Id>,
    path: &[NodeRef],
    username: Option<&str>,
) -> Option<String> {
    Some(match kind {
        "org" => format!("/org/{id}"),
        "project" => format!("/project/{id}"),
        "topic" => format!("/topic/{id}"),
        "thread" => format!("/thread/{id}"),
        "document" => format!("/doc/{id}"),
        "document_container" => format!("/document-folder/{id}"),
        "chat" => match path.last().filter(|step| Some(step.id) == node) {
            Some(step) => {
                let area = match step.kind {
                    NodeKind::Org => "org",
                    NodeKind::Project => "project",
                    NodeKind::Topic => "topic",
                    NodeKind::Thread => "thread",
                };
                format!("/{area}/{}?panel=chat&chat={id}", step.id)
            }
            None => format!("/org/{}/messages?chat={id}", org?),
        },
        "tag" => {
            // A project-defined tag has one home List. An org-defined tag needs the existing project chooser.
            let project = path.last().filter(|step| {
                step.kind == NodeKind::Project && !step.redacted && Some(step.id) == node
            })?;
            let query = form_urlencoded::Serializer::new(String::new())
                .append_pair("view", "list")
                .append_pair("tag", &serde_json::to_string(&[id]).ok()?)
                .finish();
            format!("/project/{}?{query}", project.id)
        }
        "user" => {
            let mut address = url::Url::parse("https://orch.invalid").ok()?;
            address
                .path_segments_mut()
                .ok()?
                .clear()
                .push("u")
                .push(username?);
            address.path().to_owned()
        }
        "environment" => format!("/resources/environments/{id}"),
        "provider" => format!("/resources/providers/{id}"),
        "machine" => format!("/resources/machines/{id}"),
        "agent" => format!("/resources/agents/{id}"),
        "mcp_server" => format!("/resources/mcp/{id}"),
        "host" => format!("/resources/hosts/{id}"),
        "mount" => format!("/resources/mounts/{id}"),
        "template" => format!("/resources/templates/{id}"),
        "skill" => format!("/resources/skills/{id}"),
        _ => return None,
    })
}
