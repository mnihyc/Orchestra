//! The public resource addresses and typed references accepted by the native readers.

use super::*;
pub struct Reference {
    pub target: Target,
    pub url: String,
}

pub enum Target {
    Object { kind: String, id: Id },
    Organization { address: String },
}

fn family(kind: &str) -> Option<(&'static str, &'static str)> {
    Some(match kind {
        "org" => ("org", "/org"),
        "project" => ("project", "/project"),
        "topic" => ("topic", "/topic"),
        "thread" => ("thread", "/thread"),
        "node" | "nodes" => ("node", "/nodes"),
        "doc" | "document" | "documents" => ("document", "/doc"),
        "chat" | "chats" => ("chat", "/chat"),
        "document_container" | "document-folder" => ("document_container", "/document-folder"),
        "provider" | "providers" => ("provider", "/resources/providers"),
        "agent" | "agents" => ("agent", "/resources/agents"),
        "environment" | "environments" => ("environment", "/resources/environments"),
        "host" | "hosts" => ("host", "/resources/hosts"),
        "machine" | "machines" => ("machine", "/resources/machines"),
        "template" | "templates" => ("template", "/resources/templates"),
        "mcp_server" | "mcp" | "mcp-servers" => ("mcp_server", "/resources/mcp"),
        "mount" | "mounts" => ("mount", "/resources/mounts"),
        "skill" | "skills" => ("skill", "/resources/skills"),
        _ => return None,
    })
}

pub(super) fn typed(kind: &str, id: Id) -> ApiResult<Reference> {
    let (kind, base) =
        family(kind).ok_or_else(|| denied("Use find_operation for this resource kind"))?;
    Ok(Reference {
        target: Target::Object {
            kind: kind.into(),
            id,
        },
        url: format!("{base}/{id}"),
    })
}

fn segment(value: &str) -> ApiResult<String> {
    let bad = || denied("Use a complete canonical Orch resource address");
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let hex = |byte: u8| (byte as char).to_digit(16).map(|value| value as u8);
            let high = input.next().and_then(hex).ok_or_else(bad)?;
            let low = input.next().and_then(hex).ok_or_else(bad)?;
            bytes.push((high << 4) | low);
        } else {
            bytes.push(byte);
        }
    }
    let decoded = String::from_utf8(bytes).map_err(|_| bad())?;
    if decoded.is_empty()
        || decoded == "."
        || decoded == ".."
        || decoded
            .chars()
            .any(|character| character == '/' || character == '\\' || character.is_control())
    {
        return Err(bad());
    }
    Ok(decoded)
}

pub(super) fn address(address: &url::Url) -> ApiResult<Reference> {
    let parts: Vec<_> = address.path().trim_matches('/').split('/').collect();
    let (kind, value) = match parts.as_slice() {
        [kind, value]
            if matches!(
                *kind,
                "org"
                    | "project"
                    | "topic"
                    | "thread"
                    | "node"
                    | "nodes"
                    | "doc"
                    | "document"
                    | "documents"
                    | "chat"
                    | "chats"
                    | "document-folder"
            ) =>
        {
            (*kind, *value)
        }
        ["resources", kind, value]
            if matches!(
                *kind,
                "providers"
                    | "agents"
                    | "environments"
                    | "hosts"
                    | "machines"
                    | "templates"
                    | "mcp"
                    | "mounts"
                    | "skills"
            ) =>
        {
            (*kind, *value)
        }
        _ => return Err(denied("Use a complete canonical Orch resource address")),
    };
    let value = segment(value)?;
    let target = if kind == "org" && value.parse::<Id>().is_err() {
        Target::Organization { address: value }
    } else {
        let (kind, _) =
            family(kind).ok_or_else(|| denied("Use find_operation for this resource kind"))?;
        Target::Object {
            kind: kind.into(),
            id: value
                .parse()
                .map_err(|_| denied("Use a complete Orch resource identifier"))?,
        }
    };
    let url = format!(
        "{}{}{}",
        address.path(),
        address
            .query()
            .map_or_else(String::new, |value| format!("?{value}")),
        address
            .fragment()
            .map_or_else(String::new, |value| format!("#{value}"))
    );
    Ok(Reference { target, url })
}

pub(super) fn selectors(
    kind: &str,
    address: &url::Url,
    query: &mut BTreeMap<String, Json>,
) -> ApiResult<()> {
    let key = match kind {
        "document" => "block",
        "node" => "turn",
        "chat" => "message",
        _ => return Ok(()),
    };
    let valid = |value: &str| {
        if key == "block" {
            value
                .parse::<Id>()
                .is_ok_and(|id| id.to_string().eq_ignore_ascii_case(value))
        } else {
            value
                .as_bytes()
                .first()
                .is_some_and(|first| (b'1'..=b'9').contains(first))
                && value.bytes().all(|byte| byte.is_ascii_digit())
        }
    };
    if let Some((_, value)) = address.query_pairs().find(|(name, _)| name == key)
        && valid(&value)
    {
        query.insert(key.into(), json!(value));
    }
    if let Some((fragment_key, value)) = address
        .fragment()
        .and_then(|fragment| fragment.split_once('='))
        && fragment_key == key
    {
        let value = segment(value)?;
        if valid(&value) {
            query.insert(key.into(), json!(value));
        }
    }
    Ok(())
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn typed_resources_and_public_addresses_keep_real_typed_identities() -> ApiResult<()> {
        // EN: Ask > paste resource-page address or provider reference > open source.
        // 中文：问一问 > 粘贴资源页地址或提供方引用 > 打开来源。
        let id = new_id();
        for (kind, family) in [
            ("provider", "providers"),
            ("agent", "agents"),
            ("environment", "environments"),
            ("host", "hosts"),
            ("machine", "machines"),
            ("template", "templates"),
            ("mcp_server", "mcp"),
            ("mount", "mounts"),
            ("skill", "skills"),
        ] {
            let typed = typed(kind, id)?;
            let url = url::Url::parse(&format!(
                "https://public.orch.test/resources/{family}/{id}?tab=details"
            ))
            .map_err(ApiError::internal)?;
            let public = address(&url)?;
            let Target::Object {
                kind: actual,
                id: actual_id,
            } = public.target
            else {
                panic!("an actual resource identity");
            };
            assert_eq!(actual, kind);
            assert_eq!(actual_id, id);
            assert_eq!(typed.url, format!("/resources/{family}/{id}"));
            assert_eq!(public.url, format!("/resources/{family}/{id}?tab=details"));
        }
        let org = address(
            &url::Url::parse("https://public.orch.test/org/acme.research")
                .map_err(ApiError::internal)?,
        )?;
        assert!(matches!(org.target,Target::Organization { address } if address=="acme.research"));
        for path in [
            "/resources/orgs/invalid",
            "/doc/a%2Fb",
            "/doc/a%5Cb",
            "/doc/%00",
            "/org/bad%escape",
            "/org/%ff",
        ] {
            let url = url::Url::parse(&format!("https://public.orch.test{path}"))
                .map_err(ApiError::internal)?;
            assert!(address(&url).is_err(), "{path}");
        }
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_typed_resources_and_public_addresses_keep_real_typed_identities()
    -> ::orch_api_base::middleware::requests::ApiResult<()> {
        self::typed_resources_and_public_addresses_keep_real_typed_identities()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn public_parts_use_the_same_query_and_fragment_priority_as_reference_chips() -> ApiResult<()> {
        let id = new_id();
        let block = new_id();
        let url = url::Url::parse(&format!(
            "https://public.orch.test/doc/{id}?turn=9#block={block}"
        ))
        .map_err(ApiError::internal)?;
        let mut query = BTreeMap::new();
        selectors("document", &url, &mut query)?;
        assert_eq!(query, BTreeMap::from([("block".into(), json!(block))]));
        let url = url::Url::parse(&format!(
            "https://public.orch.test/chat/{id}?message=4#message=7"
        ))
        .map_err(ApiError::internal)?;
        let mut query = BTreeMap::new();
        selectors("chat", &url, &mut query)?;
        assert_eq!(query, BTreeMap::from([("message".into(), json!("7"))]));
        Ok(())
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_public_parts_use_the_same_query_and_fragment_priority_as_reference_chips()
    -> ::orch_api_base::middleware::requests::ApiResult<()> {
        self::public_parts_use_the_same_query_and_fragment_priority_as_reference_chips()
    }
}
