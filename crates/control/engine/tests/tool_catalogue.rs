//! Exports the connector's static tool catalogue from the engine's actual declarations.
//! Run this small generator on the hosted runner when this machine cannot build the engine.

fn main() -> Result<(), serde_json::Error> {
    let tools: Vec<_> = orch_engine::tools::definitions::all(&std::collections::BTreeMap::new())
        .iter()
        .map(orch_engine::tools::definitions::ToolDefinition::to_mcp)
        .collect();
    println!("BEGIN_TOOL_CATALOGUE");
    println!("{}", serde_json::to_string_pretty(&tools)?);
    println!("END_TOOL_CATALOGUE");
    Ok(())
}
