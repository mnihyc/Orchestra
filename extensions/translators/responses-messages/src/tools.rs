//! Client tool identities remain reversible when a flat wire format has no namespace member.
use super::*;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Identity {
    namespace: Option<String>,
    name: String,
    custom: bool,
    search: bool,
}
#[derive(Clone, Default)]
pub(super) struct Tools {
    by_wire: BTreeMap<String, (Identity, Value)>,
    by_native: BTreeMap<Identity, String>,
}
fn collect(
    value: &Value,
    namespace: Option<&str>,
    description: &str,
    found: &mut BTreeMap<Identity, Value>,
) -> Result<(), TranslateError> {
    for tool in value.as_array().into_iter().flatten() {
        match tool["type"].as_str() {
            Some("namespace") => {
                let name = tool["name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| malformed("a tool namespace needs its name"))?;
                let nested =
                    namespace.map_or_else(|| name.to_owned(), |parent| format!("{parent}.{name}"));
                collect(
                    &tool["tools"],
                    Some(&nested),
                    &format!("{description}\n{}", text(tool, "description")),
                    found,
                )?;
            }
            Some("function" | "custom" | "tool_search")
                if tool["type"] != "tool_search" || tool["execution"] == "client" =>
            {
                let name = tool["name"]
                    .as_str()
                    .or_else(|| (tool["type"] == "tool_search").then_some("tool_search"))
                    .filter(|name| !name.is_empty())
                    .ok_or_else(|| malformed("a client tool needs its name"))?;
                let mut declaration = tool.clone();
                if !description.trim().is_empty() {
                    declaration["description"] =
                        json!(format!("{description}\n{}", text(tool, "description")));
                }
                found.insert(
                    Identity {
                        namespace: tool
                            .get("namespace")
                            .and_then(Value::as_str)
                            .or(namespace)
                            .map(str::to_owned),
                        name: name.to_owned(),
                        custom: tool["type"] == "custom",
                        search: tool["type"] == "tool_search",
                    },
                    declaration,
                );
            }
            _ => {} // Hosted tools keep the conversion's declared unsupported fidelity.
        }
    }
    Ok(())
}
impl Tools {
    pub fn read(request: &Value) -> Result<Self, TranslateError> {
        let mut found = BTreeMap::new();
        collect(&request["tools"], None, "", &mut found)?;
        for item in request["input"].as_array().into_iter().flatten() {
            if matches!(
                item["type"].as_str(),
                Some("additional_tools" | "tool_search_output")
            ) {
                collect(&item["tools"], None, "", &mut found)?;
            }
        }
        let mut by_wire = BTreeMap::new();
        let mut by_native = BTreeMap::new();
        let mut counts = BTreeMap::new();
        for identity in found.keys() {
            *counts.entry(identity.name.clone()).or_insert(0_usize) += 1;
        }
        for (identity, tool) in &found {
            // Messages' documented name grammar permits at most 64 ASCII letters, digits, '_' or '-'.
            let ordinary = identity.name.len() <= 64
                && identity
                    .name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                && counts.get(&identity.name) == Some(&1);
            let mut wire = identity.name.clone();
            let mut disambiguation = 0_u64;
            while !ordinary || by_wire.contains_key(&wire) {
                let digest = orch_types::Hash::of(
                    json!([
                        identity.namespace,
                        identity.name,
                        identity.custom,
                        identity.search,
                        disambiguation
                    ])
                    .to_string()
                    .as_bytes(),
                )
                .to_string();
                wire = format!("n_{}", &digest[..62]);
                if !by_wire.contains_key(&wire) {
                    break;
                }
                disambiguation = disambiguation
                    .checked_add(1)
                    .ok_or_else(|| malformed("the tool identities cannot be represented"))?;
            }
            by_native.insert(identity.clone(), wire.clone());
            by_wire.insert(wire, (identity.clone(), tool.clone()));
        }
        Ok(Self { by_wire, by_native })
    }
    pub fn declarations(&self) -> Vec<Value> {
        self.by_wire.iter().map(|(wire,(identity,tool))| {
            let mut declared=if identity.custom {
                json!({"name":wire,"description":format!("{}\nInput format: {}",text(tool,"description"),tool.get("format").unwrap_or(&Value::Null)),
                    "input_schema":{"type":"object","properties":{"input":{"type":"string"}},"required":["input"],"additionalProperties":false}})
            } else {json!({"name":wire,"description":tool.get("description").cloned().unwrap_or_else(||if identity.search{json!("Search the client tool catalogue.")}else{json!("")}),
                "input_schema":tool.get("parameters").cloned().unwrap_or(json!({"type":"object","properties":{}}))})};
            if let Some(namespace)=&identity.namespace {declared["description"]=json!(format!("Tool namespace: {namespace}.\n{}",text(&declared,"description")));}
            if let Some(strict)=tool.get("strict") {declared["strict"]=strict.clone();}
            declared
        }).collect()
    }
    pub fn wire(&self, item: &Value) -> String {
        let identity = Identity {
            namespace: item
                .get("namespace")
                .and_then(Value::as_str)
                .map(str::to_owned),
            name: item["name"].as_str().map(str::to_owned).unwrap_or_else(|| {
                if matches!(
                    item["type"].as_str(),
                    Some("tool_search" | "tool_search_call")
                ) {
                    "tool_search".into()
                } else {
                    String::new()
                }
            }),
            custom: matches!(item["type"].as_str(), Some("custom" | "custom_tool_call")),
            search: matches!(
                item["type"].as_str(),
                Some("tool_search" | "tool_search_call")
            ),
        };
        self.by_native
            .get(&identity)
            .cloned()
            .unwrap_or(identity.name)
    }
    pub fn search(&self, wire: &str) -> bool {
        self.by_wire
            .get(wire)
            .is_some_and(|(identity, _)| identity.search)
    }
    pub fn custom(&self, wire: &str) -> bool {
        self.by_wire
            .get(wire)
            .is_some_and(|(identity, _)| identity.custom)
    }
    pub fn restore(&self, wire: &str, mut item: Value) -> Value {
        if let Some((identity, _)) = self.by_wire.get(wire) {
            if identity.search {
                if let Some(item) = item.as_object_mut() {
                    item.remove("name");
                    item.remove("namespace");
                }
                return item;
            }
            item["name"] = json!(identity.name);
            if let Some(namespace) = &identity.namespace {
                item["namespace"] = json!(namespace);
            }
        }
        item
    }
}
