//! The report of an import: a row for each object and what became of it, the totals that come of the rows, and the report written out as JSON or as a table for a spreadsheet.

use super::make::Made;
use super::model::Object;
use orch_api_contract::imports::{
    ImportAction, ImportActionCounts, ImportCounts, ImportOutcome, ImportReportRow,
};
use orch_api_contract::package::PackageKind;
use orch_db::queries::imports::NewReportRow;

/// The word of an outcome as the report writes it.
#[must_use]
pub const fn outcome_word(outcome: ImportOutcome) -> &'static str {
    match outcome {
        ImportOutcome::Created => "created",
        ImportOutcome::KeptBoth => "kept_both",
        ImportOutcome::Replaced => "replaced",
        ImportOutcome::Unchanged => "unchanged",
        ImportOutcome::Skipped => "skipped",
        ImportOutcome::Failed => "failed",
    }
}

/// The word of a kind as the report writes it.
#[must_use]
pub const fn kind_word(kind: PackageKind) -> &'static str {
    match kind {
        PackageKind::Org => "org",
        PackageKind::Project => "project",
        PackageKind::Topic => "topic",
        PackageKind::Thread => "thread",
        PackageKind::Chat => "chat",
        PackageKind::Folder => "folder",
        PackageKind::Document => "document",
    }
}

/// The row of the report for an object.
#[must_use]
pub fn row_of(position: i64, object: &Object, made: &Made) -> ImportReportRow {
    ImportReportRow {
        position,
        source_id: object.object.id,
        kind: object.object.kind,
        name: object.object.name.clone(),
        path: object.path.clone(),
        action: object.action,
        outcome: made.outcome,
        made: made.made.clone(),
        message: made.message.clone(),
        code: made.code.clone(),
        params: None,
        warnings: made.warnings.clone(),
        author: made.author.clone(),
    }
}

/// A row, ready to be written.
#[must_use]
pub fn stored(row: &ImportReportRow) -> NewReportRow {
    NewReportRow {
        position: row.position,
        source_id: row.source_id,
        kind: kind_word(row.kind).to_owned(),
        outcome: outcome_word(row.outcome).to_owned(),
        warned: !row.warnings.is_empty(),
        row: serde_json::to_value(row).unwrap_or(serde_json::Value::Null),
    }
}

/// What an import came to, counted as its rows are written.
#[derive(Debug, Clone, Copy, Default)]
pub struct Counts {
    /// What it made or changed, by kind.
    pub counts: ImportCounts,
    /// What it did with its objects.
    pub actions: ImportActionCounts,
    /// How many rows ended `failed`.
    pub failed: i64,
}

impl Counts {
    /// Takes one object's outcome into the totals.
    pub fn note(&mut self, object: &Object, outcome: ImportOutcome) {
        match outcome {
            ImportOutcome::Created => self.actions.create += 1,
            ImportOutcome::KeptBoth => self.actions.keep_both += 1,
            ImportOutcome::Replaced => self.actions.replace += 1,
            ImportOutcome::Unchanged | ImportOutcome::Skipped => self.actions.skip += 1,
            ImportOutcome::Failed => {
                self.failed += 1;
                return;
            }
        }
        if matches!(outcome, ImportOutcome::Unchanged | ImportOutcome::Skipped) {
            return;
        }
        match object.object.kind {
            PackageKind::Org | PackageKind::Project | PackageKind::Topic => self.counts.places += 1,
            PackageKind::Folder => self.counts.folders += 1,
            PackageKind::Document => self.counts.documents += 1,
            PackageKind::Chat => self.counts.chats += 1,
            PackageKind::Thread => self.counts.threads += 1,
        }
    }

    /// Adds what an object carried into the totals.
    pub fn add(&mut self, files: i64, messages: i64, turns: i64, comments: i64, versions: i64) {
        self.counts.files += files;
        self.counts.messages += messages;
        self.counts.turns += turns;
        self.counts.comments += comments;
        self.counts.versions += versions;
    }
}

/// What an import was asked to do with an object, as a word.
#[must_use]
pub const fn action_word(action: ImportAction) -> &'static str {
    match action {
        ImportAction::Create => "create",
        ImportAction::KeepBoth => "keep_both",
        ImportAction::Replace => "replace",
        ImportAction::Skip => "skip",
    }
}

/// The line of headings of the report as a table for a spreadsheet.
#[must_use]
pub const fn csv_head() -> &'static str {
    "position,kind,name,path,action,outcome,message,warnings,author,made\n"
}

/// One row of the report as a line of a table for a spreadsheet, every value that is text in quotes.
#[must_use]
pub fn csv_row(row: &ImportReportRow) -> String {
    format!(
        "{},{},{},{},{},{},{},{},{},{}\n",
        row.position,
        kind_word(row.kind),
        quote(&row.name),
        quote(&row.path.join("/")),
        action_word(row.action),
        outcome_word(row.outcome),
        quote(row.message.as_deref().unwrap_or("")),
        quote(
            &row.warnings
                .iter()
                .map(|warning| warning.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        ),
        quote(row.author.as_deref().unwrap_or("")),
        row.made
            .as_ref()
            .map_or_else(String::new, |made| made.id.to_string()),
    )
}

/// A value of a table, quoted, with the quotes in it doubled, and a leading mark of a formula turned into text so that a
/// spreadsheet does not run what a document was called.
fn quote(value: &str) -> String {
    let safe = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    format!("\"{}\"", safe.replace('"', "\"\""))
}

/// The report as a table for a spreadsheet: a line of headings, then a line for each row.
#[must_use]
pub fn csv_of(rows: &[ImportReportRow]) -> String {
    let mut text = String::from(csv_head());
    for row in rows {
        text.push_str(&csv_row(row));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_with_quotes_or_the_mark_of_a_formula_is_written_so_that_a_spreadsheet_reads_it_as_text()
     {
        assert_eq!(quote("a \"b\", c"), "\"a \"\"b\"\", c\"");
        assert_eq!(quote("=SUM(A1)"), "\"'=SUM(A1)\"");
        assert_eq!(quote("plain"), "\"plain\"");
    }
}
