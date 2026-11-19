//! Public text is repaired without changing declared credentials or opaque references.
use orch_api_contract::routes::{Route, Shape};
use orch_types::Json;

pub(super) fn normalize(route: &Route, value: &mut Json) -> u64 {
    let Shape::Json(schema) = route.body else {
        return 0;
    };
    let mut generator = schemars::SchemaGenerator::default();
    let mut root = Json::from(schema(&mut generator));
    let definitions = Json::Object(generator.take_definitions(true));
    if root.is_object() {
        root["$defs"] = definitions;
    } else {
        root = serde_json::json!({"allOf":[root],"$defs":definitions});
    }
    orch_types::storable::schema::normalize(&root, value)
}
