//! Native content is classified by its contract, never by a member's spelling.
use crate::{EngineError, raw_events};
use orch_chat::redaction::Redactor;
use orch_types::{Hash, Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::OnceLock;

struct Shape(Json);
impl Shape {
    fn of<T: JsonSchema>() -> Result<Self, ()> {
        serde_json::to_value(schemars::schema_for!(T))
            .map(Self)
            .map_err(|_| ())
    }

    fn resolve<'a>(&'a self, mut schema: &'a Json) -> &'a Json {
        while let Some(reference) = schema.get("$ref").and_then(Json::as_str) {
            let Some(next) = reference
                .strip_prefix('#')
                .and_then(|path| self.0.pointer(path))
            else {
                break;
            };
            if std::ptr::eq(next, schema) {
                break;
            }
            schema = next;
        }
        schema
    }

    fn accepts(&self, schema: &Json, value: &Json) -> bool {
        let schema = self.resolve(schema);
        if let Some(expected) = schema.get("const") {
            return value == expected;
        }
        if let Some(choices) = schema.get("enum").and_then(Json::as_array) {
            return choices.contains(value);
        }
        if let Some(kind) = schema.get("type").and_then(Json::as_str) {
            let matches = match kind {
                "null" => value.is_null(),
                "object" => value.is_object(),
                "array" => value.is_array(),
                "string" => value.is_string(),
                "integer" => value.is_i64() || value.is_u64(),
                "number" => value.is_number(),
                "boolean" => value.is_boolean(),
                _ => true,
            };
            if !matches {
                return false;
            }
        }
        if let (Some(properties), Some(object)) = (
            schema.get("properties").and_then(Json::as_object),
            value.as_object(),
        ) {
            if schema
                .get("required")
                .and_then(Json::as_array)
                .is_some_and(|required| {
                    required
                        .iter()
                        .filter_map(Json::as_str)
                        .any(|name| !object.contains_key(name))
                })
            {
                return false;
            }
            for (name, property) in properties {
                let property = self.resolve(property);
                if (property.get("const").is_some() || property.get("enum").is_some())
                    && object
                        .get(name)
                        .is_some_and(|value| !self.accepts(property, value))
                {
                    return false;
                }
            }
        }
        true
    }

    fn nodes<'a>(&'a self, schema: &'a Json, value: &Json, out: &mut Vec<&'a Json>) {
        out.push(schema);
        let original = schema;
        let schema = self.resolve(schema);
        if !std::ptr::eq(original, schema) {
            out.push(schema);
        }
        if let Some(all) = schema.get("allOf").and_then(Json::as_array) {
            for child in all {
                self.nodes(child, value, out);
            }
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(child) = schema
                .get(key)
                .and_then(Json::as_array)
                .and_then(|choices| choices.iter().find(|child| self.accepts(child, value)))
            {
                self.nodes(child, value, out);
            }
        }
    }

    fn render(&self, original: &Json, masker: &Redactor) -> Presented {
        let mut out = Presented {
            value: Json::Null,
            masked: Vec::new(),
            originals: BTreeMap::new(),
        };
        out.value = self.walk(
            &[&self.0],
            original,
            masker,
            "",
            false,
            &mut out.masked,
            &mut out.originals,
        );
        out.masked.sort();
        out.masked.dedup();
        out
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the traversal carries the public path and private operand context together"
    )]
    fn walk(
        &self,
        schemas: &[&Json],
        original: &Json,
        masker: &Redactor,
        path: &str,
        inside_operand: bool,
        masked: &mut Vec<String>,
        originals: &mut BTreeMap<String, Json>,
    ) -> Json {
        let mut nodes = Vec::new();
        for schema in schemas {
            self.nodes(schema, original, &mut nodes);
        }
        let operand = !inside_operand
            && nodes
                .iter()
                .any(|node| node.get("x-native-private").and_then(Json::as_bool) == Some(true));
        if nodes
            .iter()
            .any(|node| node.get("x-native-reference").and_then(Json::as_bool) == Some(true))
        {
            return original.clone();
        }
        if nodes
            .iter()
            .any(|node| node.get("x-native-schema").and_then(Json::as_bool) == Some(true))
        {
            return form_schema(original, masker);
        }
        let value = match original {
            Json::String(text) => {
                // Only contract constants and validated reference representations are metadata.
                // An identical value in an opaque argument remains ordinary content.
                let constant = nodes.iter().any(|node| {
                    node.get("const") == Some(original)
                        || node
                            .get("enum")
                            .and_then(Json::as_array)
                            .is_some_and(|values| values.contains(original))
                });
                let identity =
                    nodes
                        .iter()
                        .any(|node| match node.get("format").and_then(Json::as_str) {
                            Some("uuid") => text.parse::<Id>().is_ok(),
                            Some("date-time") => chrono::DateTime::parse_from_rfc3339(text).is_ok(),
                            _ => {
                                node.get("x-native-content-address").and_then(Json::as_bool)
                                    == Some(true)
                                    && text.parse::<Hash>().is_ok()
                            }
                        });
                if constant || identity {
                    original.clone()
                } else {
                    let bytes = nodes.iter().any(|node| {
                        node.get("x-native-units").and_then(Json::as_str) == Some("bytes")
                    });
                    let encoded = nodes.iter().any(|node| {
                        node.get("x-native-encoded-json").and_then(Json::as_bool) == Some(true)
                    });
                    let public = if encoded {
                        masker.mask_native_json(text)
                    } else {
                        masker.mask_native(text, bytes)
                    };
                    if &public != text || public.contains('\0') {
                        masked.push(path.to_owned());
                    }
                    Json::String(public)
                }
            }
            Json::Array(values) => {
                let children: Vec<_> = nodes.iter().filter_map(|node| node.get("items")).collect();
                Json::Array(
                    values
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            self.walk(
                                &children,
                                value,
                                masker,
                                &format!("{path}/{index}"),
                                inside_operand || operand,
                                masked,
                                originals,
                            )
                        })
                        .collect(),
                )
            }
            Json::Object(values) => {
                let mut public = serde_json::Map::new();
                for (name, value) in values {
                    let mut children: Vec<_> = nodes
                        .iter()
                        .filter_map(|node| {
                            node.get("properties")
                                .and_then(|properties| properties.get(name))
                        })
                        .collect();
                    let structural = !children.is_empty();
                    if children.is_empty() {
                        children.extend(
                            nodes
                                .iter()
                                .filter_map(|node| node.get("additionalProperties")),
                        );
                    }
                    let shown = if structural {
                        name.clone()
                    } else {
                        masker.mask_native(name, false)
                    };
                    let mut unique = shown.clone();
                    let mut ordinal = 2;
                    while public.contains_key(&unique) {
                        unique = format!("{shown} ({ordinal})");
                        ordinal += 1;
                    }
                    let next = format!("{path}/{}", pointer_name(&unique));
                    if &unique != name || unique.contains('\0') {
                        masked.push(path.to_owned());
                    }
                    public.insert(
                        unique,
                        self.walk(
                            &children,
                            value,
                            masker,
                            &next,
                            inside_operand || operand,
                            masked,
                            originals,
                        ),
                    );
                }
                Json::Object(public)
            }
            _ => original.clone(),
        };
        if operand {
            let repaired = orch_types::storable::needs_repair(&value);
            // Growing byte fields have their own source tail; a storage-only repair must not pin an old snapshot here.
            if &value != original || repaired && value.is_string() {
                originals.insert(path.to_owned(), original.clone());
            }
        }
        value
    }
}

fn pointer_name(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

pub(crate) struct Presented {
    pub value: Json,
    pub masked: Vec<String>,
    originals: BTreeMap<String, Json>,
}
impl Presented {
    pub fn preserve(&mut self, path: &str, value: &Json) {
        self.originals.insert(path.to_owned(), value.clone());
        self.masked.push(path.to_owned());
    }

    pub fn private(&self, item: Id) -> Result<Option<Vec<u8>>, EngineError> {
        if self.originals.is_empty() {
            return Ok(None);
        }
        seal(&format!("native-item:{item}"), &self.originals).map(Some)
    }
}

fn shape<T: JsonSchema>(
    slot: &'static OnceLock<Result<Shape, ()>>,
) -> Result<&'static Shape, EngineError> {
    slot.get_or_init(Shape::of::<T>)
        .as_ref()
        .map_err(|()| EngineError::unreadable("native presentation types could not be read"))
}

pub(crate) fn item(kind: &str, value: &Json, masker: &Redactor) -> Result<Presented, EngineError> {
    use orch_api_contract::threads as api;
    macro_rules! render {
        ($type:ty) => {{
            static SHAPE: OnceLock<Result<Shape, ()>> = OnceLock::new();
            shape::<$type>(&SHAPE)?.render(value, masker)
        }};
    }
    Ok(match kind {
        "tool_call" => render!(api::ToolCallPayload),
        "agent_message" => render!(api::AgentMessagePayload),
        "reasoning" => render!(api::ReasoningPayload),
        "user_message" => render!(api::UserMessagePayload),
        "plan" => render!(api::PlanPayload),
        "error" => render!(api::ErrorPayload),
        "marker" => render!(api::MarkerPayload),
        _ => opaque(value, masker),
    })
}

pub(crate) fn opaque(value: &Json, masker: &Redactor) -> Presented {
    Shape(Json::Bool(true)).render(value, masker)
}

// These are JSON Schema syntax roles, not exemptions for similarly named native data.
// Field identifiers stay usable; descriptions, examples and offered values are content.
fn form_schema(value: &Json, masker: &Redactor) -> Json {
    let Json::Object(properties) = value else {
        return opaque(value, masker).value;
    };
    let mut public = serde_json::Map::new();
    for (keyword, value) in properties {
        let next = match keyword.as_str() {
            "$ref" | "required" | "dependentRequired" => value.clone(),
            "type"
                if value.as_str().is_some_and(|kind| {
                    matches!(
                        kind,
                        "null" | "boolean" | "object" | "array" | "number" | "integer" | "string"
                    )
                }) =>
            {
                value.clone()
            }
            "properties" | "$defs" | "definitions" | "patternProperties" | "dependentSchemas" => {
                value.as_object().map_or_else(
                    || opaque(value, masker).value,
                    |entries| {
                        Json::Object(
                            entries
                                .iter()
                                .map(|(name, schema)| (name.clone(), form_schema(schema, masker)))
                                .collect(),
                        )
                    },
                )
            }
            "items"
            | "additionalProperties"
            | "contains"
            | "propertyNames"
            | "not"
            | "if"
            | "then"
            | "else" => {
                if value.is_object() {
                    form_schema(value, masker)
                } else {
                    value.clone()
                }
            }
            "allOf" | "anyOf" | "oneOf" | "prefixItems" => value.as_array().map_or_else(
                || opaque(value, masker).value,
                |items| {
                    Json::Array(
                        items
                            .iter()
                            .map(|value| form_schema(value, masker))
                            .collect(),
                    )
                },
            ),
            "enum" => {
                let mut items = value.as_array().cloned().unwrap_or_default();
                let mut seen = Vec::new();
                for (index, item) in items.iter_mut().enumerate() {
                    let shown = opaque(item, masker).value;
                    *item = if seen.contains(&shown) && shown != *item {
                        Json::String(format!("{} ({})", shown.as_str().unwrap_or("*"), index + 1))
                    } else {
                        shown
                    };
                    seen.push(item.clone());
                }
                Json::Array(items)
            }
            _ => opaque(value, masker).value,
        };
        let known = matches!(
            keyword.as_str(),
            "type"
                | "format"
                | "$schema"
                | "$ref"
                | "$id"
                | "required"
                | "dependentRequired"
                | "properties"
                | "$defs"
                | "definitions"
                | "patternProperties"
                | "dependentSchemas"
                | "items"
                | "additionalProperties"
                | "contains"
                | "propertyNames"
                | "not"
                | "if"
                | "then"
                | "else"
                | "allOf"
                | "anyOf"
                | "oneOf"
                | "prefixItems"
                | "enum"
                | "const"
                | "default"
                | "description"
                | "title"
                | "minimum"
                | "maximum"
                | "exclusiveMinimum"
                | "exclusiveMaximum"
                | "multipleOf"
                | "minLength"
                | "maxLength"
                | "minItems"
                | "maxItems"
                | "uniqueItems"
                | "minProperties"
                | "maxProperties"
                | "pattern"
                | "examples"
                | "readOnly"
                | "writeOnly"
        );
        let name = if known {
            keyword.clone()
        } else {
            masker.mask_native(keyword, false)
        };
        let mut unique = name.clone();
        let mut ordinal = 2;
        while public.contains_key(&unique) {
            unique = format!("{name} ({ordinal})");
            ordinal += 1;
        }
        public.insert(unique, next);
    }
    Json::Object(public)
}

pub(crate) fn error(value: &Json, masker: &Redactor) -> Result<Json, EngineError> {
    static SHAPE: OnceLock<Result<Shape, ()>> = OnceLock::new();
    let mut rendered = shape::<orch_link_contract::events::TurnError>(&SHAPE)?
        .render(value, masker)
        .value;
    if let Some(code) = value.get("code").and_then(Json::as_str).and_then(|code| {
        code.parse::<orch_link_contract::events::TurnErrorCode>()
            .ok()
    }) {
        rendered["code"] = Json::String(code.as_str().to_owned());
    }
    if let Some(limit) = value.get("limit").filter(|value| !value.is_null()) {
        if serde_json::from_value::<orch_api_contract::threads::TurnLimit>(limit.clone()).is_ok() {
            static LIMIT: OnceLock<Result<Shape, ()>> = OnceLock::new();
            rendered["limit"] = shape::<orch_api_contract::threads::TurnLimit>(&LIMIT)?
                .render(limit, masker)
                .value;
        } else {
            // Unrecognized vendor facts remain readable diagnostics, never an invented plan action.
            let detail = opaque(limit, masker).value.to_string();
            let said = rendered
                .get("said")
                .and_then(Json::as_str)
                .filter(|said| !said.is_empty());
            rendered["said"] = Json::String(
                said.map_or_else(|| detail.clone(), |said| format!("{said}\n{detail}")),
            );
            if let Some(object) = rendered.as_object_mut() {
                object.remove("limit");
            }
        }
    }
    Ok(rendered)
}

pub(crate) fn approval(value: &Json, masker: &Redactor) -> Result<Json, EngineError> {
    static SHAPE: OnceLock<Result<Shape, ()>> = OnceLock::new();
    let mut public = shape::<orch_api_contract::approvals::HarnessRequest>(&SHAPE)?
        .render(value, masker)
        .value;
    if let Some(questions) = public
        .pointer_mut("/detail/questions")
        .and_then(Json::as_array_mut)
    {
        for question in questions {
            if let Some(options) = question.get_mut("options").and_then(Json::as_array_mut) {
                let mut used = std::collections::BTreeSet::new();
                for option in options {
                    if let Some(label) = option
                        .get("label")
                        .and_then(Json::as_str)
                        .map(str::to_owned)
                    {
                        let mut shown = label.clone();
                        let mut ordinal = 2;
                        while !used.insert(shown.clone()) {
                            shown = format!("{label} ({ordinal})");
                            ordinal += 1;
                        }
                        option["label"] = Json::String(shown);
                    }
                }
            }
        }
    }
    Ok(public)
}

pub(crate) fn decision(value: &Json, masker: &Redactor) -> Result<Json, EngineError> {
    static SHAPE: OnceLock<Result<Shape, ()>> = OnceLock::new();
    let mut public = shape::<orch_api_contract::approvals::ApprovalDecision>(&SHAPE)?
        .render(value, masker)
        .value;
    if let Some(answers) = value.get("answers").and_then(Json::as_object) {
        // Question identifiers are selection references; each answer remains content.
        public["answers"] = Json::Object(
            answers
                .iter()
                .map(|(id, answer)| (id.clone(), opaque(answer, masker).value))
                .collect(),
        );
    }
    orch_types::storable::normalize_json(&mut public);
    Ok(public)
}

/// Offered values are selected in their public form and delivered in their private form.
pub(crate) fn restore_answer(public: &Json, original: &Json, answer: &mut Json) {
    if let (Some(shown), Some(actual)) = (
        public.get("enum").and_then(Json::as_array),
        original.get("enum").and_then(Json::as_array),
    ) && let Some(index) = shown.iter().position(|value| value == answer)
        && let Some(value) = actual.get(index)
    {
        *answer = value.clone();
        return;
    }
    for key in ["const", "default"] {
        if let (Some(shown), Some(actual)) = (public.get(key), original.get(key))
            && shown == answer
        {
            *answer = actual.clone();
            return;
        }
    }
    match answer {
        Json::Object(values) => {
            for (key, answer) in values {
                if let (Some(shown), Some(actual)) = (
                    public.get("properties").and_then(|values| values.get(key)),
                    original
                        .get("properties")
                        .and_then(|values| values.get(key)),
                ) {
                    restore_answer(shown, actual, answer);
                }
            }
        }
        Json::Array(values) => {
            if let (Some(shown), Some(actual)) = (public.get("items"), original.get("items")) {
                for answer in values {
                    restore_answer(shown, actual, answer);
                }
            }
        }
        _ => {}
    }
}

pub(crate) async fn approval_original(
    conn: &mut orch_db::Conn,
    id: Id,
    public: &Json,
) -> Result<Json, EngineError> {
    let cipher: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT native_request FROM approvals WHERE id=$1")
            .bind(id)
            .fetch_one(conn)
            .await
            .map_err(orch_db::DbError::from)?;
    cipher.map_or_else(
        || Ok(public.clone()),
        |bytes| open(&format!("native-approval:{id}"), &bytes),
    )
}

pub(crate) async fn instruction(
    conn: &mut orch_db::Conn,
    segment: Id,
    value: &Json,
    masker: &Redactor,
) -> Result<Json, EngineError> {
    static SHAPE: OnceLock<Result<Shape, ()>> = OnceLock::new();
    let rendered =
        shape::<orch_agent_contract::session::InstructionFile>(&SHAPE)?.render(value, masker);
    let hidden = rendered
        .masked
        .iter()
        .any(|path| path == "/path" || path == "/parent_path");
    let mut public = rendered.value;
    let identity = serde_json::json!({"kind":value.get("kind"),"path":value.get("path"),"label":value.get("label"),"home":value.get("home")});
    let fingerprint = Hash::of(identity.to_string().as_bytes()).to_string();
    let existing: Option<Id> = sqlx::query_scalar(
        "SELECT source_id FROM native_instruction_operands WHERE segment_id=$1 AND source_hash=$2",
    )
    .bind(segment)
    .bind(&fingerprint)
    .fetch_optional(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let id = existing.unwrap_or_else(orch_types::new_id);
    let private = if rendered.masked.is_empty() {
        None
    } else {
        Some(seal(&format!("native-instruction:{id}"), value)?)
    };
    sqlx::query("INSERT INTO native_instruction_operands(segment_id,source_hash,source_id,data) VALUES($1,$2,$3,$4) ON CONFLICT(segment_id,source_hash) DO UPDATE SET data=excluded.data")
        .bind(segment).bind(&fingerprint).bind(id).bind(private).execute(&mut *conn).await.map_err(orch_db::DbError::from)?;
    // A source address containing hidden text is not a public file door.
    // The private source remains with the native program; its label stays readable.
    public["environment"] = if hidden {
        Json::Null
    } else {
        value.get("environment").cloned().unwrap_or(Json::Null)
    };
    public["scope"] = Json::Null;
    public["path_redacted"] = Json::Bool(hidden);
    public["source_key"] = serde_json::json!(id);
    orch_types::storable::normalize_json(&mut public);
    Ok(public)
}

/// Session routing identifiers remain operational metadata. All other native fields
/// pass this projection before a handler can persist or announce them. Item content,
/// approvals, traces and tool execution use their typed private/public boundary below.
pub(crate) fn event(
    original: &orch_link_contract::events::Event,
    masker: &Redactor,
) -> Result<orch_link_contract::events::Event, EngineError> {
    use orch_link_contract::events::Event;
    if matches!(
        original,
        Event::InstructionSources(_)
            | Event::ItemStarted(_)
            | Event::ItemUpdated(_)
            | Event::ItemCompleted(_)
            | Event::ItemDelta(_)
            | Event::ApprovalRequested(_)
            | Event::RawFrame(_)
            | Event::OrchToolCall(_)
    ) {
        return Ok(original.clone());
    }
    static SHAPE: OnceLock<Result<Shape, ()>> = OnceLock::new();
    let source = serde_json::to_value(original)
        .map_err(|_| EngineError::unreadable("a native event could not be prepared"))?;
    let mut public = shape::<Event>(&SHAPE)?.render(&source, masker).value;
    if let Event::TurnCompleted(completed) = original
        && let Some(failure) = &completed.error
    {
        public["b"]["error"] = error(&serde_json::to_value(failure)?, masker)?;
    }
    Event::decode(
        original.kind().as_str(),
        public.get("b").cloned().unwrap_or(Json::Null),
    )
    .map_err(|_| EngineError::unreadable("a native event's public projection could not be read"))
}

pub(crate) fn seal<T: Serialize>(binding: &str, value: &T) -> Result<Vec<u8>, EngineError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| EngineError::unreadable("private native data could not be prepared"))?;
    raw_events::master_key()
        .map_err(|_| EngineError::unreadable("private native data cannot be sealed"))?
        .seal(&bytes, binding.as_bytes())
        .map_err(|_| EngineError::unreadable("private native data could not be sealed"))
}

pub(crate) fn open<T: for<'a> Deserialize<'a>>(
    binding: &str,
    bytes: &[u8],
) -> Result<T, EngineError> {
    let bytes = raw_events::master_key()
        .map_err(|_| EngineError::unreadable("private native data cannot be opened"))?
        .open(bytes, binding.as_bytes())
        .map_err(|_| EngineError::unreadable("private native data could not be opened"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| EngineError::unreadable("private native data could not be read"))
}

pub(crate) async fn originals(
    conn: &mut orch_db::Conn,
    item: Id,
    public: &Json,
) -> Result<Json, EngineError> {
    let cipher: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT data FROM native_item_operands WHERE item_id=$1")
            .bind(item)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    let mut restored = public.clone();
    if let Some(cipher) = cipher {
        let patches: BTreeMap<String, Json> = open(&format!("native-item:{item}"), &cipher)?;
        for (path, value) in patches {
            if let Some(target) = restored.pointer_mut(&path) {
                *target = value;
            }
        }
    }
    if let Some(object) = restored.as_object_mut() {
        object.remove("masked_fields");
    }
    Ok(restored)
}

pub(crate) async fn keep_originals(
    conn: &mut orch_db::Conn,
    item: Id,
    cipher: Option<&[u8]>,
) -> Result<(), EngineError> {
    if let Some(cipher) = cipher {
        sqlx::query("INSERT INTO native_item_operands(item_id,data) VALUES($1,$2) ON CONFLICT(item_id) DO UPDATE SET data=excluded.data")
            .bind(item).bind(cipher).execute(conn).await.map_err(orch_db::DbError::from)?;
    } else {
        sqlx::query("DELETE FROM native_item_operands WHERE item_id=$1")
            .bind(item)
            .execute(conn)
            .await
            .map_err(orch_db::DbError::from)?;
    }
    Ok(())
}

/// Decoding errors can quote a native value, so diagnostics carry only trusted source metadata.
pub(crate) fn unreadable_event(thread: Id, sequence: i64) {
    tracing::warn!(thread = %thread, sequence, "a native event could not be read and was skipped");
}
