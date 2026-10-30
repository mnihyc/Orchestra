//! What a spec can be proposed from, the fields each import asks for, and the notes it returns.
//!
//! A template can be written in its form, and it can be **imported**: made from something a person already
//! has. An import proposes a spec and saves nothing; the form opens filled in, with a note for everything
//! that was read and everything that could not be carried over, and the person saves it as any template.
//! Which things can be imported is declared by each driver ([`ImportDef`]): a format, a title, and the
//! fields it asks for, in the shape every form uses. The `docker` driver declares three: a Dockerfile, an
//! image reference and an image archive. An import is a convenience and no second kind of template: what is
//! saved is an ordinary spec, editable like any other. It is the same for a template of either pool.

use orch_types::fields::{FieldError, validate};
use orch_types::{Field, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::runtime::RuntimeKind;

/// One thing a driver can propose a spec from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ImportDef {
    /// The format's name, such as `dockerfile`.
    pub format: String,
    /// What it is called.
    pub title: String,
    /// What a person gives for it.
    pub fields: Vec<Field>,
}

impl ImportDef {
    /// Checks what a person gave against the fields of the format: every required field is answered,
    /// every answer fits its field, nothing else is given. Every fault is returned.
    pub fn check_values(&self, values: &Json) -> Result<(), Vec<FieldError>> {
        validate(&self.fields, values)
    }
}

/// A request to propose a spec: the body of `node.template.import`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ImportRequest {
    /// The pool the template is for.
    pub kind: RuntimeKind,
    /// One of the formats the driver declares.
    pub format: String,
    /// What the person gave, as the format's fields ask.
    pub values: Json,
}

/// How serious a note of an import is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoteLevel {
    /// Something that was read.
    Info,
    /// Something that could not be carried over, or that the person should look at.
    Warning,
    /// Something that makes the proposal unusable here, such as an image no host can run.
    Blocker,
}

/// One note of an import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImportNote {
    /// How serious it is.
    pub level: NoteLevel,
    /// What it says.
    pub text: String,
}

/// What an import proposes: a spec, and a note for everything that was read and everything that could not
/// be carried over. Nothing is created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ImportProposal {
    /// The proposed spec, in the driver's own schema for the pool.
    pub spec: Json,
    /// The notes.
    pub notes: Vec<ImportNote>,
}

impl ImportProposal {
    /// Whether any note is a blocker: the proposal cannot be used as it is.
    #[must_use]
    pub fn is_blocked(&self) -> bool {
        self.notes
            .iter()
            .any(|note| note.level == NoteLevel::Blocker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::FieldType;
    use serde_json::json;

    fn dockerfile() -> ImportDef {
        let mut text = Field::new("dockerfile", "The Dockerfile", FieldType::SecretText);
        text.max_bytes = Some(262_144);
        let mut context = Field::new("git", "A Git address for its context", FieldType::Text);
        context.required = false;
        ImportDef {
            format: "dockerfile".into(),
            title: "A Dockerfile".into(),
            fields: vec![text, context],
        }
    }

    #[test]
    fn an_import_is_offered_with_the_fields_it_asks_for_and_what_is_given_is_checked_against_them()
    {
        let import = dockerfile();
        assert_eq!(
            import.check_values(&json!({ "dockerfile": "FROM x" })),
            Ok(())
        );
        let faults = import
            .check_values(&json!({ "git": "https://example.com/r.git", "unknown": 1 }))
            .unwrap_err();
        let paths: Vec<_> = faults.iter().map(|fault| fault.path.as_str()).collect();
        assert_eq!(paths, ["dockerfile", "unknown"]);
    }

    #[test]
    fn a_proposal_with_a_blocker_cannot_be_used_and_notes_keep_their_level() {
        let proposal: ImportProposal = serde_json::from_value(json!({
            "spec": { "image": "ghcr.io/o/x:1" },
            "notes": [
                { "level": "info", "text": "the last USER is agent" },
                { "level": "warning", "text": "declared as a volume: it will not outlive the unit unless you add a mount for it" },
                { "level": "blocker", "text": "no host has that architecture" } ]
        }))
        .unwrap();
        assert!(proposal.is_blocked());
        assert_eq!(proposal.notes[1].level, NoteLevel::Warning);
        let fine = ImportProposal {
            spec: json!({}),
            notes: vec![],
        };
        assert!(!fine.is_blocked());
    }

    #[test]
    fn a_request_to_import_reads_as_the_node_command_writes_it() {
        let request: ImportRequest = serde_json::from_value(
            json!({ "kind": "provider", "format": "image", "values": { "reference": "gcr.io/distroless/static" } }),
        )
        .unwrap();
        assert_eq!(request.kind, RuntimeKind::Provider);
    }
}
