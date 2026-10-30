//! Print the connector's shipped Orch tool catalogue from its canonical engine definitions.

use std::collections::BTreeMap;

fn main() -> Result<(), serde_json::Error> {
    let tools: Vec<_> = orch_engine::tools::definitions::all(&BTreeMap::new())
        .iter()
        .map(orch_engine::tools::definitions::ToolDefinition::to_mcp)
        .collect();
    println!("{}", serde_json::to_string_pretty(&tools)?);
    Ok(())
}
