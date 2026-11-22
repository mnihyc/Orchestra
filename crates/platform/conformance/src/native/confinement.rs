//! The real program reports no tools and refuses a forced action on the controlled endpoint.
use super::*;

pub(super) async fn exercise(
    adapter: &mut dyn AgentAdapter,
    fixture: &Fixture<'_>,
    events: &mut (impl Stream<Item = AdapterEvent> + Unpin),
    spec: &SessionSpec,
) -> Result<(), String> {
    let before = (fixture.requests)().len();
    let seen = completed(
        adapter,
        events,
        spec,
        "#!mock\nsay Supplied material reached the model.\n",
    )
    .await?;
    let inventory = seen
        .iter()
        .find_map(|event| match event {
            SessionEvent::RawFrame(raw)
                if raw.payload["type"] == "system" && raw.payload["subtype"] == "init" =>
            {
                Some(&raw.payload)
            }
            _ => None,
        })
        .ok_or_else(|| "the native program did not report its actual tool inventory".to_owned())?;
    if !inventory["tools"].as_array().is_some_and(Vec::is_empty)
        || !inventory["mcp_servers"]
            .as_array()
            .is_some_and(Vec::is_empty)
    {
        return Err("the program exposed tools in a conversation that must have none".into());
    }
    let marker = fixture.workspace.join("forbidden-tool-free-action");
    let command = format!("printf forbidden > '{}'", marker.display());
    let script = format!(
        "#!mock\n{}\nthen\nsay The forbidden tool is unavailable.\n",
        tool_line(fixture, "command", &json!({"command":command}))?
    );
    let attempts = completed(adapter, events, spec, &script).await?;
    if marker.exists()
        || attempts
            .iter()
            .any(|event| matches!(event, SessionEvent::ApprovalRequested(_)))
    {
        return Err("a conversation with no tools executed or requested an action".into());
    }
    let requests = (fixture.requests)();
    if requests.len() <= before
        || requests[before..].iter().any(|request| {
            request.body.get("tools").is_some_and(|tools| {
                !tools.is_null() && !tools.as_array().is_some_and(Vec::is_empty)
            })
        })
    {
        return Err("the controlled model did not receive an empty tool catalogue".into());
    }
    Ok(())
}
