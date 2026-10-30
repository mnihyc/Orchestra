//! What changed between two versions, block by block, with who wrote it, people and agents alike; and who last changed each block.
//!
//! Two states of a document are compared block by block, by the identifier a
//! block keeps from the moment it is typed. A block that is in the later state
//! alone was inserted, one in the earlier alone was removed, and one in both
//! is listed when its type, its props, its content or its place changed;
//! blocks that did not are left out. Each says who changed it last between the
//! two: that is on the block itself, so that what an agent did to a page is as
//! clear as what a person did, with nothing kept for agents that is not kept
//! for people. Who a removal is owed to is not in the later state, and is asked
//! of the caller.

use std::collections::HashMap;

use orch_api_contract::documents::{
    Author, DocumentBlock, DocumentBlockChange, DocumentTitleChange,
};
use orch_types::Id;

use crate::blocks::Outline;
use crate::position::Position;

fn same_place(a: &DocumentBlock, b: &DocumentBlock) -> bool {
    a.parent_id == b.parent_id && Position::parse(&a.position) == Position::parse(&b.position)
}

fn differs(before: &DocumentBlock, after: &DocumentBlock) -> bool {
    before.block_type != after.block_type
        || before.props != after.props
        || before.content != after.content
        || !same_place(before, after)
}

/// The title, when it differs between the two states.
#[must_use]
pub fn title(before: &str, after: &str) -> Option<DocumentTitleChange> {
    (before != after).then(|| DocumentTitleChange {
        before: before.to_owned(),
        after: after.to_owned(),
    })
}

/// The blocks that differ between the live blocks `before` and `after`: those
/// of the later state in the order of the page, then those it lost in the order
/// they stood. `removed_by` says who took a block out, for the blocks that are
/// in `before` alone; where it knows no one, the block's last author before
/// is given.
#[must_use]
pub fn between(
    before: &[DocumentBlock],
    after: &[DocumentBlock],
    removed_by: &dyn Fn(Id) -> Option<Author>,
) -> Vec<DocumentBlockChange> {
    let earlier: HashMap<Id, &DocumentBlock> = before
        .iter()
        .filter(|block| !block.removed)
        .map(|block| (block.id, block))
        .collect();
    let later: HashMap<Id, &DocumentBlock> = after
        .iter()
        .filter(|block| !block.removed)
        .map(|block| (block.id, block))
        .collect();
    let mut changes = Vec::new();
    for (block, _) in Outline::new(after).walk() {
        let then = earlier.get(&block.id).copied();
        if then.is_none_or(|then| differs(then, block)) {
            changes.push(DocumentBlockChange {
                id: block.id,
                before: then.cloned(),
                after: Some(block.clone()),
                by: block.updated_by.clone(),
            });
        }
    }
    for (block, _) in Outline::new(before).walk() {
        if !later.contains_key(&block.id) {
            changes.push(DocumentBlockChange {
                id: block.id,
                before: Some(block.clone()),
                after: None,
                by: removed_by(block.id).or_else(|| block.updated_by.clone()),
            });
        }
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::documents::{AuthorAgent, BlockContent, BlockProps, Inline, NoProps};
    use orch_api_contract::nodes::{NodeKind, NodeRef};
    use orch_api_contract::users::{Avatar, UserKind, UserRef};
    use orch_types::new_id;

    fn person(name: &str) -> Author {
        Author::User(UserRef {
            id: new_id(),
            kind: UserKind::Human,
            username: name.to_owned(),
            address: None,
            display_name: name.to_owned(),
            avatar: Avatar {
                text: name.chars().take(1).collect(),
                color: 1,
            },
        })
    }

    fn agent() -> Author {
        Author::Agent(AuthorAgent {
            agent: "codex".into(),
            thread: NodeRef {
                thread_display: None,
                id: new_id(),
                kind: NodeKind::Thread,
                name: Some("Fix".into()),
                redacted: false,
            },
        })
    }

    fn block(
        id: Id,
        parent: Option<Id>,
        position: &str,
        words: &str,
        by: &Author,
    ) -> DocumentBlock {
        let content = BlockContent::Inline(vec![Inline::Text {
            text: words.to_owned(),
            marks: None,
            href: None,
            color: None,
            background: None,
        }]);
        DocumentBlock {
            id,
            document_id: Id::nil(),
            parent_id: parent,
            position: position.to_owned(),
            block_type: orch_api_contract::documents::BlockType::Paragraph,
            props: BlockProps::Empty(NoProps {}),
            content,
            removed: false,
            seq_created: 1,
            seq_updated: 1,
            updated_by: Some(by.clone()),
            updated_at: "2026-10-08T17:04:05.123Z".parse().unwrap(),
        }
    }

    fn nobody(_: Id) -> Option<Author> {
        None
    }

    #[test]
    fn blocks_that_did_not_change_are_left_out_and_the_rest_say_what_they_were_and_are_and_who() {
        let (ada, codex) = (person("ada"), agent());
        let (kept, edited, inserted, removed) = (new_id(), new_id(), new_id(), new_id());
        let before = vec![
            block(kept, None, "1", "same", &ada),
            block(edited, None, "2", "old words", &ada),
            block(removed, None, "3", "doomed", &ada),
        ];
        let after = vec![
            block(kept, None, "1", "same", &ada),
            block(edited, None, "2", "new words", &codex),
            block(inserted, None, "2.5", "added", &ada),
        ];
        let by_bo = person("bo");
        let changes = between(&before, &after, &|id| {
            (id == removed).then(|| by_bo.clone())
        });
        let ids: Vec<Id> = changes.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [edited, inserted, removed],
            "page order, then what was lost"
        );
        assert!(changes[0].before.is_some() && changes[0].after.is_some());
        assert_eq!(
            changes[0].by,
            Some(codex),
            "an agent is named as a person is"
        );
        assert!(changes[1].before.is_none());
        assert!(changes[2].after.is_none());
        assert_eq!(changes[2].by, Some(by_bo));
    }

    #[test]
    fn a_block_moved_without_a_change_of_words_is_a_change() {
        let ada = person("ada");
        let (a, b) = (new_id(), new_id());
        let before = vec![
            block(a, None, "1", "a", &ada),
            block(b, None, "2", "b", &ada),
        ];
        let after = vec![
            block(a, None, "3", "a", &ada),
            block(b, None, "2", "b", &ada),
        ];
        let changes = between(&before, &after, &nobody);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].id, a);
        // A position written another way is not a move.
        let same = vec![
            block(a, None, "1.0", "a", &ada),
            block(b, None, "2", "b", &ada),
        ];
        assert!(between(&before, &same, &nobody).is_empty());
        // Nor is a block that stands under another parent unchanged.
        let under = vec![
            block(a, Some(b), "1", "a", &ada),
            block(b, None, "2", "b", &ada),
        ];
        assert_eq!(between(&before, &under, &nobody).len(), 1);
    }

    #[test]
    fn a_removal_is_owed_to_the_last_author_when_nobody_knows_who_took_it_out() {
        let ada = person("ada");
        let a = new_id();
        let before = vec![block(a, None, "1", "a", &ada)];
        let changes = between(&before, &[], &nobody);
        assert_eq!(changes[0].by, Some(ada));
    }

    #[test]
    fn a_title_is_a_change_only_when_it_differs() {
        assert_eq!(title("Plan", "Plan"), None);
        let change = title("Plan", "Plan B").unwrap();
        assert_eq!(
            (change.before.as_str(), change.after.as_str()),
            ("Plan", "Plan B")
        );
    }
}
