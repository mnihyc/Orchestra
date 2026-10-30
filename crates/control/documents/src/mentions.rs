//! People and things named in a document; the one notice a first mention sends.
//!
//! Naming a person alone sends no notice. A chip that names a node or a
//! document is shown to each reader by name where they may see the thing,
//! and without one where not.

use orch_api_contract::documents::{BlockContent, Inline, InlineRefKind};
use orch_types::Id;

/// The people named in inline content, once each, in the order they appear.
#[must_use]
pub fn in_inline(runs: &[Inline]) -> Vec<Id> {
    let mut named = Vec::new();
    for run in runs {
        if let Inline::Mention { user_id, .. } = run
            && !named.contains(user_id)
        {
            named.push(*user_id);
        }
    }
    named
}

/// The people named in a block's content, once each: in its text, or in the
/// cells of its table.
#[must_use]
pub fn in_content(content: &BlockContent) -> Vec<Id> {
    match content {
        BlockContent::Inline(runs) => in_inline(runs),
        BlockContent::Rows(rows) => {
            let mut named = Vec::new();
            for cell in rows.iter().flat_map(|row| row.cells.iter()) {
                for user in in_inline(&cell.content) {
                    if !named.contains(&user) {
                        named.push(user);
                    }
                }
            }
            named
        }
    }
}

/// The nodes and documents that inline content has chips for.
#[must_use]
pub fn chips_in(runs: &[Inline]) -> Vec<(InlineRefKind, Id)> {
    runs.iter()
        .filter_map(|run| match run {
            Inline::Ref { kind, id, .. } => Some((*kind, *id)),
            _ => None,
        })
        .collect()
}

/// Whom to tell: those of `named` who were not told before and who may read
/// the document (`may_read`), in the order they were named.
#[must_use]
pub fn to_tell(named: &[Id], told: &[Id], may_read: impl Fn(Id) -> bool) -> Vec<Id> {
    named
        .iter()
        .copied()
        .filter(|person| !told.contains(person) && may_read(*person))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::documents::TableCell;
    use orch_api_contract::documents::TableRow;
    use orch_types::new_id;

    fn mention(user: Id) -> Inline {
        Inline::Mention {
            user_id: user,
            user: None,
        }
    }

    #[test]
    fn a_person_named_twice_in_a_block_is_named_once() {
        let (ada, bo) = (new_id(), new_id());
        let runs = [mention(ada), mention(bo), mention(ada)];
        assert_eq!(in_inline(&runs), [ada, bo]);
    }

    #[test]
    fn the_cells_of_a_table_name_people_as_text_does() {
        let (ada, bo) = (new_id(), new_id());
        let rows = BlockContent::Rows(vec![TableRow {
            cells: vec![
                TableCell {
                    content: vec![mention(bo)],
                    ..TableCell::default()
                },
                TableCell {
                    content: vec![mention(ada), mention(bo)],
                    ..TableCell::default()
                },
            ],
        }]);
        assert_eq!(in_content(&rows), [bo, ada]);
    }

    #[test]
    fn nobody_is_told_twice_and_nobody_who_could_not_open_the_document_is_told() {
        let (ada, bo, cy) = (new_id(), new_id(), new_id());
        let told = [ada];
        let readers = [ada, bo];
        let tell = to_tell(&[ada, bo, cy], &told, |person| readers.contains(&person));
        assert_eq!(tell, [bo], "ada was told, and cy cannot read it");
    }

    #[test]
    fn a_chip_is_found_by_what_it_names() {
        let (node, doc) = (new_id(), new_id());
        let runs = [
            Inline::Ref {
                display: None,
                path: None,
                kind: InlineRefKind::Node,
                id: node,
                part: None,
                name: None,
                label: None,
            },
            Inline::Break,
            Inline::Ref {
                display: None,
                path: None,
                kind: InlineRefKind::Document,
                id: doc,
                part: None,
                name: None,
                label: None,
            },
        ];
        assert_eq!(
            chips_in(&runs),
            [(InlineRefKind::Node, node), (InlineRefKind::Document, doc)]
        );
    }
}
