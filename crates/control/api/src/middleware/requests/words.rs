//! Public problem sentences reuse the interface catalogue; diagnostic identifiers stay in structured members and logs.

use orch_api_contract::common::{ErrorCode, Problem};
use orch_types::Json;

fn words(code: ErrorCode) -> String {
    orch_api_contract::messages::for_code(code)
}

fn has_code(message: &str) -> bool {
    message
        .split(|character: char| {
            !character.is_ascii_alphanumeric() && character != '_' && character != '.'
        })
        .any(|token| {
            token
                .chars()
                .any(|character| character.is_ascii_alphabetic())
                && (token.contains('_') || token.trim_matches('.').contains('.'))
        })
}

/// A problem's detail always has its catalogue sentence and recovery act. A fault's path still
/// places the control, while its useful explanation is kept unless it contains diagnostic syntax.
pub(super) fn prepare(problem: &mut Problem) {
    // Flattened extras cannot overwrite the contract fields (some native failures used
    // an extra `detail`, producing a second JSON member with the unreviewed diagnostics).
    for reserved in ["type", "code", "title", "status", "detail", "request_id"] {
        problem.extra.remove(reserved);
    }
    problem.title = problem.code.title().to_owned();
    problem.detail = words(problem.code);
    if let Some(message) = problem.extra.get_mut("message") {
        *message = Json::String(words(problem.code));
    }
    if let Some(faults) = problem.extra.get_mut("errors").and_then(Json::as_array_mut) {
        for fault in faults {
            if let Some(message) = fault.get_mut("message")
                && message
                    .as_str()
                    .is_none_or(|message| message.trim().is_empty() || has_code(message))
            {
                *message = Json::String(words(ErrorCode::RequestInvalid));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extras_cannot_replace_contract_fields_or_publish_internal_diagnostics() {
        let diagnostic = "private_diagnostic";
        let mut problem = Problem::new(ErrorCode::AuthzDenied, diagnostic)
            .with("required", ["thread.interact"])
            .with("message", diagnostic);
        problem.request_id = "request-for-support".to_owned();
        let original = problem.clone();
        let reserved = ["type", "code", "title", "status", "detail", "request_id"];
        for member in reserved {
            problem.extra.insert(member.to_owned(), json!(diagnostic));
        }
        prepare(&mut problem);
        assert_eq!(problem.code, original.code);
        assert_eq!(problem.kind, original.kind);
        assert_eq!(problem.status, original.status);
        assert_eq!(problem.request_id, original.request_id);
        assert_eq!(problem.extra["required"], original.extra["required"]);
        assert!(
            reserved
                .iter()
                .all(|member| !problem.extra.contains_key(*member))
        );
        for message in [
            problem.title.as_str(),
            problem.detail.as_str(),
            problem.extra["message"].as_str().unwrap(),
        ] {
            assert!(!message.is_empty());
            assert!(!message.contains(diagnostic));
        }
        let prepared = problem.clone();
        prepare(&mut problem);
        assert_eq!(problem, prepared);
    }

    #[test]
    fn field_diagnostics_are_withheld_without_losing_the_field_or_useful_explanation() {
        let useful = json!({"path": "name", "message": "a useful explanation"});
        for diagnostic in [
            Json::Null,
            json!(42),
            json!(""),
            json!("internal_column_check"),
        ] {
            let mut problem = Problem::new(ErrorCode::RequestInvalid, "").with(
                "errors",
                [
                    json!({"path": "home_node_id", "message": diagnostic}),
                    useful.clone(),
                ],
            );
            prepare(&mut problem);
            let faults = problem.extra["errors"].as_array().unwrap();
            assert_eq!(faults[0]["path"], "home_node_id");
            let message = faults[0]["message"].as_str().unwrap();
            assert!(!message.is_empty());
            assert!(!has_code(message));
            assert_eq!(faults[1], useful);
        }
    }
}
