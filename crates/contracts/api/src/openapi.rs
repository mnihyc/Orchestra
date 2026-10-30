//! Render OpenAPI from contract declarations without API serving dependencies.
//!
//! The OpenAPI 3.1 document of the API is written from the declarations of the contract, the same
//! ones the binder serves from: each request becomes an operation with its parameters, its body,
//! its answer and who may make it, and every type goes in as the JSON Schema it derives. It is
//! extended with wire schemas supplied by the caller before definitions are collected.

use crate::common::{BASE_PATH, PROBLEM_JSON, Problem};
use crate::routes::{Access, Route, Shape};
use schemars::{SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value, json};

/// Where the definitions of the types are in the document.
const COMPONENTS: &str = "/components/schemas";

fn generator() -> SchemaGenerator {
    // A schema says what is written: a member that may be `null` is always there, one that may be
    // absent is left out. That is the reading a client needs of a reply, and it is how the bodies
    // are declared too.
    let mut settings = SchemaSettings::draft2020_12().for_serialize();
    settings.definitions_path = format!("#{COMPONENTS}/").into();
    settings.meta_schema = None;
    SchemaGenerator::new(settings)
}

/// Follows a reference into the definitions, so that a query type can be taken apart into
/// parameters.
fn resolve<'a>(schema: &'a Value, definitions: &'a Map<String, Value>) -> &'a Value {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.rsplit('/').next())
        .and_then(|name| definitions.get(name))
        .unwrap_or(schema)
}

fn parameters(
    route: &Route,
    query: Option<&Value>,
    definitions: &Map<String, Value>,
) -> Vec<Value> {
    let mut out: Vec<Value> = route
        .parameters()
        .into_iter()
        .map(|name| json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } }))
        .collect();
    if let Some(query) = query {
        let object = resolve(query, definitions);
        let required: Vec<&str> = object
            .get("required")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for (name, schema) in object
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            out.push(json!({ "name": name, "in": "query", "required": required.contains(&name.as_str()), "schema": schema }));
        }
    }
    out
}

fn access(route: &Route) -> (&'static str, Option<&'static str>, Option<&'static str>) {
    match route.access {
        Access::Public => ("public", None, None),
        Access::Cookie => ("cookie", None, None),
        Access::User => ("user", None, None),
        Access::Session => ("session", None, None),
        Access::Permission(key) => ("permission", Some(key), None),
        Access::Rule(rule) => ("rule", None, Some(rule)),
    }
}

/// The OpenAPI document of `routes`, for the release `version`.
///
/// `extend_schemas` registers additional wire types after route schemas and before definitions
/// are collected, without making this crate depend on the contracts that supply those types.
#[must_use]
pub fn document(
    routes: &[Route],
    version: &str,
    extend_schemas: fn(&mut SchemaGenerator),
) -> Value {
    let mut generator = generator();
    let problem = generator.subschema_for::<Problem>();
    // A job's generic result carries these cleanup receipts outside a route reply schema.
    let _cleanup_outcome = generator.subschema_for::<crate::storage::CleanupOutcome>();
    let mut operations: Vec<(Route, Value)> = Vec::new();
    for route in routes {
        let schema_of = |shape: Shape, generator: &mut SchemaGenerator| match shape {
            Shape::Json(write) => Some(Value::from(write(generator))),
            _ => None,
        };
        let query = schema_of(route.query, &mut generator);
        let body = schema_of(route.body, &mut generator);
        let reply = schema_of(route.reply, &mut generator);
        let (kind, permission, rule) = access(route);
        let mut operation = json!({
            "operationId": route.name,
            "summary": route.summary.trim(),
            "x-orch-access": kind,
            "x-orch-tokens": route.access.tokens(),
            "responses": {},
        });
        if let Some(permission) = permission {
            operation["x-orch-permission"] = json!(permission);
        }
        if let Some(rule) = rule {
            operation["x-orch-rule"] = json!(rule);
        }
        if route.access.signed_in() {
            operation["security"] = json!([{ "bearer": [] }]);
        }
        match (route.body, body) {
            (Shape::Json(_), Some(schema)) => {
                operation["requestBody"] = json!({ "required": true, "content": { "application/json": { "schema": schema } } });
            }
            (Shape::Bytes, _) => {
                operation["requestBody"] = json!({ "required": true, "content": { "application/octet-stream": { "schema": { "type": "string", "format": "binary" } } } });
            }
            _ => {}
        }
        let success = match (route.reply, reply) {
            (Shape::Json(_), Some(schema)) => {
                json!({ "description": "Done.", "content": { "application/json": { "schema": schema } } })
            }
            (Shape::Raw, _) => {
                json!({ "description": "What the request says it answers with: a file, a redirect, text, a stream or a socket." })
            }
            _ => json!({ "description": "Done; nothing to return." }),
        };
        operation["responses"][route.status.to_string()] = success;
        for (status, write) in route.responses {
            operation["responses"][status.to_string()] = json!({
                "description": "Done.",
                "content": { "application/json": { "schema": Value::from(write(&mut generator)) } }
            });
        }
        operation["responses"]["default"] = json!({ "description": "A problem, with a code a client can act on.", "content": { PROBLEM_JSON: { "schema": problem.clone() } } });
        // Parameters are filled in below, once every definition exists.
        operation["x-query"] = query.unwrap_or(Value::Null);
        operations.push((*route, operation));
    }

    // The frames of the browser's socket travel no declared request, but the browser's types are
    // generated from this one document.
    let _ = generator.subschema_for::<crate::search::SearchStreamEvent>();
    extend_schemas(&mut generator);
    let definitions: Map<String, Value> = generator.take_definitions(true);
    let mut paths = Map::new();
    for (route, mut operation) in operations {
        let query = operation
            .as_object_mut()
            .and_then(|object| object.remove("x-query"))
            .filter(|query| !query.is_null());
        let parameters = parameters(&route, query.as_ref(), &definitions);
        if !parameters.is_empty() {
            operation["parameters"] = Value::Array(parameters);
        }
        let entry = paths
            .entry(route.path.to_owned())
            .or_insert_with(|| json!({}));
        entry[route.method.as_str().to_lowercase()] = operation;
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Orch API",
            "version": version,
            "description": "Every request of the Orch HTTP API. Generated from the route registry; do not edit.",
            "license": { "name": "Apache-2.0", "identifier": "Apache-2.0" },
        },
        "servers": [{ "url": BASE_PATH }],
        "paths": paths,
        "components": {
            "schemas": definitions,
            "securitySchemes": { "bearer": { "type": "http", "scheme": "bearer", "description": "An access token of a browser session, or an API token." } },
        },
    })
}
