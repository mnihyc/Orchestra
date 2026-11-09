//! General tools preserve the native query vocabulary and its page boundaries.

use super::*;

use serde::de::DeserializeOwned;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Projection {
    DocumentAfter { version: i64 },
}

impl Projection {
    pub(super) fn apply(&self, data: &mut Json) -> ApiResult<()> {
        match self {
            Self::DocumentAfter { version } => {
                let rows = data
                    .get_mut("items")
                    .and_then(Json::as_array_mut)
                    .ok_or_else(|| ApiError::internal("the native history page has no entries"))?;
                let newest = rows
                    .iter()
                    .filter_map(|row| row.get("version").and_then(Json::as_i64))
                    .max()
                    .unwrap_or(0);
                for row in rows.iter() {
                    if !matches!(row.get("version"), Some(Json::Null))
                        && row.get("version").and_then(Json::as_i64).is_none()
                    {
                        return Err(ApiError::internal(
                            "the native history entry has no version",
                        ));
                    }
                }
                rows.retain(|row| match row.get("version").and_then(Json::as_i64) {
                    Some(number) => number > *version,
                    None => newest >= *version,
                });
                if let Some(cursor) = data.get("next_cursor").and_then(Json::as_str) {
                    let next: i64 = cursor.parse().map_err(ApiError::internal)?;
                    if next <= *version {
                        data["next_cursor"] = Json::Null;
                    }
                }
            }
        }
        Ok(())
    }
}

fn query<Q: DeserializeOwned + Serialize>(
    values: BTreeMap<String, Json>,
) -> ApiResult<BTreeMap<String, Json>> {
    let value = serde_json::to_value(values).map_err(ApiError::internal)?;
    let typed: Q = serde_json::from_value(value)
        .map_err(|_| denied("The filter does not match this native list's query schema. Use find_operation to inspect it."))?;
    serde_json::from_value(serde_json::to_value(typed).map_err(ApiError::internal)?)
        .map_err(ApiError::internal)
}

pub(super) fn list(
    kind: &str,
    place: Id,
    filter: Option<&Json>,
) -> ApiResult<(String, ReadParameters)> {
    let mut values: BTreeMap<String, Json> = match filter {
        None => BTreeMap::new(),
        Some(filter) => serde_json::from_value(filter.clone())
            .map_err(|_| denied("A list filter is an object"))?,
    };
    let (route, values) = match kind {
        "thread" | "threads" => (
            orch_api_contract::topics::TopicSyncGet::ROUTE,
            query::<orch_api_contract::topics::TopicSyncGetQuery>(values)?,
        ),
        "doc" | "document" | "documents" => (
            orch_api_contract::documents::NodeDocumentList::ROUTE,
            query::<orch_api_contract::documents::NodeDocumentListQuery>(values)?,
        ),
        "places" => (
            orch_api_contract::nodes::OrgTreeGet::ROUTE,
            query::<orch_api_contract::nodes::OrgTreeGetQuery>(values)?,
        ),
        "topic" | "topics" => (
            orch_api_contract::nodes::TopicList::ROUTE,
            query::<orch_api_contract::nodes::TopicListQuery>(values)?,
        ),
        "org" | "orgs" | "project" | "projects" => {
            let selected = if matches!(kind, "org" | "orgs") {
                "org"
            } else {
                "project"
            };
            if values
                .get("kind")
                .is_some_and(|value| !value.is_null() && value.as_str() != Some(selected))
            {
                return Err(denied("The filter kind differs from the requested list"));
            }
            values.insert("kind".into(), json!(selected));
            (
                orch_api_contract::nodes::NodeChildList::ROUTE,
                query::<orch_api_contract::nodes::NodeChildListQuery>(values)?,
            )
        }
        _ => return Err(denied("Use find_operation to select a list for this kind")),
    };
    Ok((
        route.name.into(),
        ReadParameters {
            path: path(place),
            query: values,
            ..Default::default()
        },
    ))
}

fn version(value: Option<&Json>) -> ApiResult<Option<i64>> {
    match value {
        None | Some(Json::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            .filter(|version| *version >= 0)
            .map(Some)
            .ok_or_else(|| denied("A document history boundary is a nonnegative version number")),
    }
}

pub(super) fn history(
    kind: &str,
    id: Id,
    args: &Json,
) -> ApiResult<(ReadParameters, Option<Projection>)> {
    let mut query = BTreeMap::new();
    let projection = if matches!(kind, "doc" | "document") {
        if let Some(before) = args.get("before").filter(|value| !value.is_null()) {
            let cursor = match before {
                Json::String(value) => value.clone(),
                value if value.as_i64().is_some() => value.to_string(),
                _ => {
                    return Err(denied(
                        "A document history cursor is text or a version number",
                    ));
                }
            };
            query.insert("cursor".into(), json!(cursor));
        }
        version(args.get("after"))?.map(|version| Projection::DocumentAfter { version })
    } else {
        for key in ["before", "after"] {
            if let Some(value) = args.get(key).filter(|value| !value.is_null()) {
                query.insert(key.into(), value.clone());
            }
        }
        None
    };
    Ok((
        ReadParameters {
            path: path(id),
            query,
            ..Default::default()
        },
        projection,
    ))
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn list_filters_retain_the_native_query_and_cannot_change_the_list_kind() -> ApiResult<()> {
        // EN: Ask > list documents with one row > continue its native cursor.
        // 中文：问一问 > 每页列出一份文档 > 使用原生游标读取下一页。
        let place = new_id();
        let (operation, parameters) = list(
            "documents",
            place,
            Some(&json!({"limit":1,"cursor":"next-native-page","under":true})),
        )?;
        assert_eq!(operation, "NodeDocumentList");
        let query: orch_api_contract::documents::NodeDocumentListQuery =
            serde_json::from_value(json!(parameters.query)).map_err(ApiError::internal)?;
        assert_eq!(query.limit, Some(1));
        assert_eq!(query.cursor.as_deref(), Some("next-native-page"));
        assert_eq!(query.under, Some(true));
        assert_eq!(parameters.path, path(place));
        assert!(list("documents", place, Some(&json!({"unknown_filter":true}))).is_err());
        assert!(list("projects", place, Some(&json!({"kind":"thread"}))).is_err());
        let (operation, parameters) = list(
            "projects",
            place,
            Some(&json!({"limit":2,"archived":"include"})),
        )?;
        assert_eq!(operation, "NodeChildList");
        assert_eq!(parameters.query["kind"], "project");
        assert_eq!(parameters.query["archived"], "include");
        assert_eq!(default_operation(&operation)?, "NodeChildList.read");
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_list_filters_retain_the_native_query_and_cannot_change_the_list_kind()
    -> crate::middleware::requests::ApiResult<()> {
        self::list_filters_retain_the_native_query_and_cannot_change_the_list_kind()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn document_history_keeps_native_cursor_and_replays_its_exclusive_lower_bound() -> ApiResult<()>
    {
        // EN: Ask > document history after version 2 > continue before version 4;
        // refresh the same tool result after a new cut without showing older entries.
        // 中文：问一问 > 读取版本 2 之后的文档历史 > 继续读取版本 4 之前的记录；
        // 新版本生成后重新读取同一结果，不能混入较早记录。
        let id = new_id();
        let (parameters, projection) = history("document", id, &json!({"before":"4","after":2}))?;
        assert_eq!(
            parameters.query,
            BTreeMap::from([("cursor".into(), json!("4"))])
        );
        let query: orch_api_contract::documents::DocumentVersionListQuery =
            serde_json::from_value(json!(parameters.query)).map_err(ApiError::internal)?;
        assert_eq!(query.cursor.as_deref(), Some("4"));
        let trace = Trace {
            operation: "DocumentVersionList.read".into(),
            parameters,
            projection,
        };
        let saved: Trace =
            serde_json::from_value(serde_json::to_value(trace).map_err(ApiError::internal)?)
                .map_err(ApiError::internal)?;
        let Some(projection) = saved.projection else {
            panic!("the lower bound is retained in the real read witness");
        };
        let mut page =
            json!({"items":[{"version":3},{"version":2},{"version":1}],"next_cursor":"1"});
        projection.apply(&mut page)?;
        assert_eq!(page, json!({"items":[{"version":3}],"next_cursor":null}));
        let mut newer =
            json!({"items":[{"version":null},{"version":6},{"version":5}],"next_cursor":"5"});
        projection.apply(&mut newer)?;
        assert_eq!(newer["items"].as_array().map(Vec::len), Some(3));
        assert_eq!(newer["next_cursor"], "5");
        let mut future = json!({"items":[{"version":null},{"version":1}],"next_cursor":null});
        projection.apply(&mut future)?;
        assert_eq!(future["items"], json!([]));
        let (cursor, _) = history("doc", id, &json!({"before":"opaque-native-cursor"}))?;
        assert_eq!(cursor.query["cursor"], "opaque-native-cursor");
        assert!(history("doc", id, &json!({"after":-1})).is_err());
        assert!(history("doc", id, &json!({"after":1.5})).is_err());
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_document_history_keeps_native_cursor_and_replays_its_exclusive_lower_bound()
    -> crate::middleware::requests::ApiResult<()> {
        self::document_history_keeps_native_cursor_and_replays_its_exclusive_lower_bound()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn thread_and_chat_history_keep_their_native_sequence_windows() -> ApiResult<()> {
        for kind in ["thread", "chat"] {
            let (parameters, projection) = history(kind, new_id(), &json!({"before":21}))?;
            assert_eq!(
                parameters.query,
                BTreeMap::from([("before".into(), json!(21))])
            );
            assert!(projection.is_none());
        }
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_thread_and_chat_history_keep_their_native_sequence_windows()
    -> crate::middleware::requests::ApiResult<()> {
        self::thread_and_chat_history_keep_their_native_sequence_windows()
    }
}
