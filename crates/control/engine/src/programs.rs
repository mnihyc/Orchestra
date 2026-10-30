//! What an agent program and its executors are called, as payloads carry them: the kind with its name in each language, and an executor with its tool set.
//!
//! Nothing here names a program. A program is a manifest in the registry; where the registry does not know a kind,
//! the name its profile's descriptor declared says what it is called, and the kind itself where nothing does.

use orch_agent_programs::{Localized, Programs};
use orch_api_contract::programs::{ExecutorRef, ToolSetInfo};
use orch_api_contract::resources::HarnessKindName;
use orch_types::Json;

/// What a program is called, as a payload carries it: its kind, and its name in English and in the other languages.
///
/// What its profile's descriptor declares (`declared`) says first, since a system may rename a program in the
/// profile's overrides; then the manifest of the registry, which is known before the program runs; then the kind
/// itself. A name is never absent. The names in the other languages are the descriptor's (`text`), and the
/// manifest's where the descriptor leaves the name as the manifest has it.
#[must_use]
pub fn harness_name(programs: &Programs, kind: &str, declared: Option<&Json>) -> HarnessKindName {
    let manifest = programs.program(kind);
    let declared_name = declared
        .and_then(|descriptor| descriptor.pointer("/harness/name"))
        .and_then(Json::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned);
    let listed = declared
        .and_then(|descriptor| descriptor.get("text"))
        .and_then(Json::as_object)
        .map(|languages| {
            languages
                .iter()
                .filter_map(|(language, words)| {
                    Some((
                        language.clone(),
                        words.get("harness.name")?.as_str()?.to_owned(),
                    ))
                })
                .collect::<std::collections::BTreeMap<String, String>>()
        })
        .unwrap_or_default();
    let renamed = match (&declared_name, manifest) {
        (Some(declared), Some(manifest)) => declared != manifest.name(),
        _ => false,
    };
    let mut names = if renamed {
        std::collections::BTreeMap::new()
    } else {
        manifest
            .map(|manifest| manifest.names.others())
            .unwrap_or_default()
    };
    names.extend(listed);
    HarnessKindName {
        kind: kind.to_owned(),
        name: declared_name
            .or_else(|| manifest.map(|manifest| manifest.name().to_owned()))
            .unwrap_or_else(|| kind.to_owned()),
        names,
    }
}

/// What a program is called where only its kind and the name its profile's descriptor declared are at hand.
#[must_use]
pub fn named_by(kind: &str, declared_name: Option<&str>) -> HarnessKindName {
    let declared = declared_name.map(|name| serde_json::json!({ "harness": { "name": name } }));
    harness_name(Programs::builtin(), kind, declared.as_ref())
}

/// [`harness_name`] for the shipped programs alone, where no more is at hand.
#[must_use]
pub fn shipped_name(kind: &str, declared: Option<&Json>) -> HarnessKindName {
    harness_name(Programs::builtin(), kind, declared)
}

/// A tool set of an executor, as the API says it.
#[must_use]
pub fn tool_set_info(set: &orch_agent_programs::ToolSetManifest) -> ToolSetInfo {
    ToolSetInfo {
        name: set.name.clone(),
        title: set.title.clone(),
        summary: set.summary.clone(),
    }
}

/// The executor of an arrangement named, with the tool set it works with. None where the harness is its own executor.
#[must_use]
pub fn executor_ref(
    programs: &Programs,
    kind: Option<&str>,
    tool_set: Option<&str>,
) -> Option<ExecutorRef> {
    let kind = kind?;
    let known = programs.executor(kind);
    Some(ExecutorRef {
        kind: kind.to_owned(),
        title: known.map_or_else(
            || Localized::english(kind),
            |executor| executor.title.clone(),
        ),
        tool_set: tool_set.map(|name| {
            known
                .and_then(|executor| executor.tool_set(name))
                .map_or_else(
                    || ToolSetInfo {
                        name: name.to_owned(),
                        title: Localized::english(name),
                        summary: Localized::english(name),
                    },
                    tool_set_info,
                )
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_shipped_program_is_named_in_each_language_and_a_stranger_by_its_own_words_or_its_kind() {
        let codex = shipped_name("codex", None);
        assert_eq!(
            (codex.kind.as_str(), codex.name.as_str()),
            ("codex", "Codex")
        );
        let fake = shipped_name("fake", None);
        assert_eq!(fake.in_language("zh-CN"), "示例代理");
        let declared = json!({ "harness": { "name": "Third" }, "text": { "zh-CN": { "harness.name": "第三个" } } });
        let third = shipped_name("third", Some(&declared));
        assert_eq!(third.name, "Third");
        assert_eq!(third.in_language("zh-CN"), "第三个");
        assert_eq!(shipped_name("unheard-of", None).name, "unheard-of");
    }

    #[test]
    fn an_arrangements_executor_is_named_with_its_tool_set() {
        let named = executor_ref(Programs::builtin(), Some("orch-exec"), Some("plain")).unwrap();
        assert_eq!(named.title.en(), "Orch tools");
        assert_eq!(named.tool_set.unwrap().title.en(), "Basic tools");
        assert!(executor_ref(Programs::builtin(), None, None).is_none());
        let unknown = executor_ref(Programs::builtin(), Some("theirs"), None).unwrap();
        assert_eq!(unknown.title.en(), "theirs");
    }
}
