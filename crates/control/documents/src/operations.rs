//! Batches of changes to blocks (insert, update, move, remove), each applied whole; a change written on an older state of a block refused, with what the block is now.
//!
//! A batch is applied to the tree of a document in memory, and only when
//! every operation has been, are the effects handed to whoever stores them:
//! a batch that fails changes nothing. Before any operation is tried, every
//! block the batch builds on is compared with what is stored: a block that
//! was changed or removed since the batch was written on it makes the batch
//! stale, and all such blocks are named together, so that the writer gets
//! all of what they missed in one answer. Two people who write in different
//! blocks never meet.
//!
//! A block that an earlier operation of the same batch made or changed is
//! the writer's own: a later operation of the batch may build on it without
//! knowing the stamp it will be given.

use std::collections::{HashMap, HashSet};

use orch_api_contract::documents::{
    BlockContent, BlockType, DOCUMENT_OPS_MAX, DocumentOp, NewBlock,
};
use orch_types::{Id, Seq};

use crate::blocks::{self, Checked, Fault};
use crate::mentions;
use crate::position::Position;
use crate::versions::{BlockChanges, Tally};

/// A block as a batch sees it.
#[derive(Debug, Clone)]
pub struct Node {
    /// Its identifier.
    pub id: Id,
    /// The block it is under; none at the top level.
    pub parent: Option<Id>,
    /// Where it stands among its siblings.
    pub position: Position,
    /// Its type.
    pub block_type: BlockType,
    /// The sequence at which it last changed.
    pub seq_updated: Seq,
    /// Whether it is gone: such a row is here only to be answered with.
    pub removed: bool,
    /// Its props and content, where they were read; they are for every block
    /// that an `update` names.
    pub checked: Option<Checked>,
}

/// The blocks of a document that a batch can build on.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    nodes: HashMap<Id, Node>,
    children: HashMap<Option<Id>, Vec<Id>>,
}

impl Tree {
    /// The tree of `nodes`: the live blocks of the document, and the rows of
    /// removed blocks that an operation names.
    #[must_use]
    pub fn new(nodes: Vec<Node>) -> Self {
        let mut tree = Self::default();
        for node in nodes {
            tree.nodes.insert(node.id, node);
        }
        let mut children: HashMap<Option<Id>, Vec<Id>> = HashMap::new();
        for node in tree.nodes.values().filter(|node| !node.removed) {
            children.entry(node.parent).or_default().push(node.id);
        }
        for siblings in children.values_mut() {
            siblings.sort_by(|a, b| tree.order(*a, *b));
        }
        tree.children = children;
        tree
    }

    /// Siblings are ordered by position, and blocks put at the same place by
    /// their identifiers.
    fn order(&self, a: Id, b: Id) -> std::cmp::Ordering {
        match (self.nodes.get(&a), self.nodes.get(&b)) {
            (Some(left), Some(right)) => left.position.cmp(&right.position).then_with(|| a.cmp(&b)),
            _ => a.cmp(&b),
        }
    }

    /// Puts a block into its parent's children, where its position says.
    fn link(&mut self, id: Id) {
        let Some(parent) = self.nodes.get(&id).map(|node| node.parent) else {
            return;
        };
        let mut siblings = self.children.remove(&parent).unwrap_or_default();
        let at = siblings.partition_point(|other| self.order(*other, id).is_lt());
        siblings.insert(at, id);
        self.children.insert(parent, siblings);
    }

    fn unlink(&mut self, id: Id) {
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        if let Some(siblings) = self.children.get_mut(&node.parent) {
            siblings.retain(|other| *other != id);
        }
    }

    /// Supplies a freshly read node while retaining the rest of an edit's tree.
    /// Parent membership and sibling order follow the supplied live or removed state.
    pub fn supply(&mut self, node: Node) {
        let id = node.id;
        let placement_changed = self.nodes.get(&id).is_none_or(|previous| {
            previous.parent != node.parent
                || previous.position != node.position
                || previous.removed != node.removed
        });
        if placement_changed {
            self.unlink(id);
        }
        let live = !node.removed;
        self.nodes.insert(id, node);
        if placement_changed && live {
            self.link(id);
        }
    }

    /// Releases hydrated content after a stored chunk, retaining only its identities, positions and sequence stamps.
    /// A later update supplies that block again before applying its changes.
    pub fn discard_data(&mut self, ids: &[Id]) {
        for id in ids {
            if let Some(node) = self.nodes.get_mut(id) {
                node.checked = None;
            }
        }
    }

    /// The block `id`, live or not.
    #[must_use]
    pub fn get(&self, id: Id) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// The block `id`, if it is live.
    #[must_use]
    pub fn live(&self, id: Id) -> Option<&Node> {
        self.nodes.get(&id).filter(|node| !node.removed)
    }

    /// The live blocks directly under `parent`, in order.
    #[must_use]
    pub fn kids(&self, parent: Option<Id>) -> &[Id] {
        self.children.get(&parent).map_or(&[], Vec::as_slice)
    }

    /// How many live blocks there are.
    #[must_use]
    pub fn live_count(&self) -> usize {
        self.nodes.values().filter(|node| !node.removed).count()
    }

    /// How many blocks `id` stands under.
    #[must_use]
    pub fn depth_of(&self, id: Id) -> usize {
        let mut depth = 0;
        let mut at = self.nodes.get(&id).and_then(|node| node.parent);
        while let Some(parent) = at {
            depth += 1;
            if depth > self.nodes.len() {
                break;
            }
            at = self.nodes.get(&parent).and_then(|node| node.parent);
        }
        depth
    }

    /// How many levels of blocks lie under `id`.
    #[must_use]
    pub fn height_of(&self, id: Id) -> usize {
        self.kids(Some(id))
            .iter()
            .map(|child| 1 + self.height_of(*child))
            .max()
            .unwrap_or(0)
    }

    /// Whether `id` is `ancestor` or stands anywhere under it.
    #[must_use]
    pub fn is_within(&self, id: Id, ancestor: Id) -> bool {
        let mut at = Some(id);
        let mut steps = 0;
        while let Some(current) = at {
            if current == ancestor {
                return true;
            }
            steps += 1;
            if steps > self.nodes.len() {
                return false;
            }
            at = self.nodes.get(&current).and_then(|node| node.parent);
        }
        false
    }

    fn descendants(&self, id: Id) -> Vec<Id> {
        let mut out = Vec::new();
        let mut stack: Vec<Id> = self.kids(Some(id)).to_vec();
        while let Some(next) = stack.pop() {
            out.push(next);
            stack.extend_from_slice(self.kids(Some(next)));
        }
        out
    }

    /// Where a block goes under `parent`: first among its children when
    /// `after` is none; right after `after` when that is one of them; at the
    /// end when it is not (it is gone, or stands elsewhere). `skip` is a block
    /// that is being moved and so is no neighbour of its own.
    fn place(&self, parent: Option<Id>, after: Option<Id>, skip: Option<Id>) -> Position {
        let siblings: Vec<&Node> = self
            .kids(parent)
            .iter()
            .filter(|id| Some(**id) != skip)
            .filter_map(|id| self.nodes.get(id))
            .collect();
        let at_the_end = || {
            siblings
                .last()
                .map_or_else(Position::first, |last| Position::after_last(&last.position))
        };
        let Some(after) = after else {
            return siblings.first().map_or_else(Position::first, |first| {
                Position::before_first(&first.position)
            });
        };
        match siblings.iter().position(|node| node.id == after) {
            Some(at) => match siblings.get(at + 1) {
                Some(next) => Position::between(&siblings[at].position, &next.position),
                None => Position::after_last(&siblings[at].position),
            },
            None => at_the_end(),
        }
    }
}

/// A block as it is to be written.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    /// Its identifier.
    pub id: Id,
    /// The block it is under.
    pub parent: Option<Id>,
    /// Where it stands.
    pub position: Position,
    /// Its type, props, content, plain text and size.
    pub checked: Checked,
}

/// Where a block was put.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    /// The block.
    pub id: Id,
    /// The block it is under now.
    pub parent: Option<Id>,
    /// Where it stands now.
    pub position: Position,
}

/// What a batch did, for whoever stores it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Effects {
    /// The blocks to insert, a parent before the blocks under it.
    pub inserted: Vec<Written>,
    /// The blocks whose type, props or content changed.
    pub updated: Vec<Written>,
    /// The blocks put elsewhere.
    pub moved: Vec<Moved>,
    /// The blocks that were live and are gone, with everything under them.
    pub removed: Vec<Id>,
    /// Every block whose row changes, to be stamped with the batch's sequence.
    pub touched: Vec<Id>,
    /// How many of each.
    pub tally: Tally,
    /// Whether no block that was live before the batch is live after it.
    pub replaces_everything: bool,
    /// The people named in what was inserted or updated, once each.
    pub named: Vec<Id>,
    /// The files of the document that the written blocks name.
    pub files: Vec<(Id, Id)>,
}

impl Effects {
    /// The block identities to merge into the document's history span.
    #[must_use]
    pub fn block_changes(&self) -> BlockChanges {
        BlockChanges {
            inserted: self.inserted.iter().map(|block| block.id).collect(),
            updated: self.updated.iter().map(|block| block.id).collect(),
            moved: self.moved.iter().map(|block| block.id).collect(),
            removed: self.removed.iter().copied().collect(),
        }
    }
}

/// Why a batch was not applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Something in it is wrong: where, and what.
    Invalid(Vec<Fault>),
    /// Blocks it builds on were changed or removed since: those blocks.
    Stale(Vec<Id>),
    /// A block it makes has an identifier that is in use.
    Exists(Id),
    /// One block would be larger than one may be.
    BlockTooLarge {
        /// Where.
        path: String,
        /// How large it is.
        bytes: i64,
    },
    /// The document would have more blocks than it may.
    TooManyBlocks {
        /// What it may.
        limit: i64,
    },
    /// A block would be nested deeper than a block may be.
    TooDeep {
        /// Where.
        path: String,
    },
    /// The tree was not given what the batch needs; a fault of the caller.
    NotLoaded(Id),
}

fn invalid(path: impl Into<String>, message: impl Into<String>) -> Refusal {
    Refusal::Invalid(vec![Fault::at(path, message)])
}

/// What a batch refers to, so that it can be loaded before it is applied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    /// Blocks an `update` names: their props and content are needed.
    pub updated: Vec<Id>,
    /// Every block the operations name, as a target, a parent or a sibling:
    /// their rows are needed, a removed block's among them.
    pub named: Vec<Id>,
    /// The identifiers of every block the batch makes.
    pub made: Vec<Id>,
}

fn made_in(block: &NewBlock, into: &mut Vec<Id>) {
    into.push(block.id);
    for child in block.children.iter().flatten() {
        made_in(child, into);
    }
}

/// What `ops` refer to.
#[must_use]
pub fn references(ops: &[DocumentOp]) -> References {
    let mut found = References::default();
    for op in ops {
        match op {
            DocumentOp::Insert {
                block,
                parent_id,
                after_id,
            } => {
                made_in(block, &mut found.made);
                found.named.extend(parent_id);
                found.named.extend(after_id);
            }
            DocumentOp::Update { id, .. } => {
                found.updated.push(*id);
                found.named.push(*id);
            }
            DocumentOp::Move {
                id,
                parent_id,
                after_id,
            } => {
                found.named.push(*id);
                found.named.extend(parent_id);
                found.named.extend(after_id);
            }
            DocumentOp::Remove { id } => found.named.push(*id),
        }
    }
    found
}

struct Run<'a> {
    tree: &'a mut Tree,
    seq: Seq,
    /// Blocks made by the batch, in the order they were put in.
    made: Vec<Id>,
    made_set: HashSet<Id>,
    /// Blocks that were live before the batch and whose content changed.
    updated: Vec<Id>,
    /// Blocks that were live before the batch and were put elsewhere.
    moved: Vec<Id>,
    /// Blocks that were live before the batch and are gone.
    removed: Vec<Id>,
    /// Containers whose columns were touched.
    columns: HashSet<Id>,
}

fn new_block_height(block: &NewBlock) -> usize {
    block
        .children
        .iter()
        .flatten()
        .map(|child| 1 + new_block_height(child))
        .max()
        .unwrap_or(0)
}

impl Run<'_> {
    fn stamp(&mut self, id: Id) {
        if let Some(node) = self.tree.nodes.get_mut(&id) {
            node.seq_updated = self.seq;
        }
    }

    /// Checks a block to be made and the blocks under it.
    fn check_new(
        &self,
        block: &NewBlock,
        parent: Option<BlockType>,
        path: &str,
        checked: &mut Vec<(Id, Checked, Option<Id>)>,
        parent_id: Option<Id>,
        seen: &mut HashSet<Id>,
    ) -> Result<(), Refusal> {
        if self.tree.get(block.id).is_some() || !seen.insert(block.id) {
            return Err(Refusal::Exists(block.id));
        }
        if let Err(why) = blocks::may_hold(parent, block.block_type) {
            return Err(invalid(format!("{path}.type"), why));
        }
        let outcome = blocks::check(
            block.block_type,
            block.props.as_ref(),
            block.content.as_ref(),
        );
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(faults) => {
                return Err(Refusal::Invalid(
                    faults
                        .into_iter()
                        .map(|fault| Fault::at(format!("{path}.{}", fault.path), fault.message))
                        .collect(),
                ));
            }
        };
        if !blocks::fits_a_block(outcome.bytes) {
            return Err(Refusal::BlockTooLarge {
                path: path.to_owned(),
                bytes: outcome.bytes,
            });
        }
        checked.push((block.id, outcome, parent_id));
        for (at, child) in block.children.iter().flatten().enumerate() {
            self.check_new(
                child,
                Some(block.block_type),
                &format!("{path}.children[{at}]"),
                checked,
                Some(block.id),
                seen,
            )?;
        }
        Ok(())
    }

    fn insert(
        &mut self,
        index: usize,
        block: &NewBlock,
        parent: Option<Id>,
        after: Option<Id>,
    ) -> Result<(), Refusal> {
        let path = format!("ops[{index}].block");
        let parent_type = match parent {
            None => None,
            Some(id) => match self.tree.live(id) {
                Some(node) => Some(node.block_type),
                None => {
                    return Err(invalid(
                        format!("ops[{index}].parent_id"),
                        "was removed earlier in this batch",
                    ));
                }
            },
        };
        let depth = parent.map_or(0, |id| self.tree.depth_of(id) + 1);
        if depth + new_block_height(block) > blocks::nesting_limit() {
            return Err(Refusal::TooDeep { path });
        }
        let mut checked = Vec::new();
        self.check_new(
            block,
            parent_type,
            &path,
            &mut checked,
            parent,
            &mut HashSet::new(),
        )?;
        let top = self.tree.place(parent, after, None);
        // The blocks under the new one are numbered from one, in the order given.
        let mut next_child: HashMap<Id, u64> = HashMap::new();
        for (id, outcome, parent_id) in checked {
            let position = match parent_id {
                Some(under) if id != block.id => {
                    let counter = next_child.entry(under).or_insert(0);
                    *counter += 1;
                    Position::nth(*counter)
                }
                _ => top.clone(),
            };
            let node = Node {
                id,
                parent: parent_id,
                position,
                block_type: outcome.block_type,
                seq_updated: self.seq,
                removed: false,
                checked: Some(outcome),
            };
            self.tree.nodes.insert(id, node);
            self.tree.link(id);
            self.made.push(id);
            self.made_set.insert(id);
        }
        if let Some(parent) = parent {
            self.columns.insert(parent);
        }
        if block.block_type == BlockType::Columns {
            self.columns.insert(block.id);
        }
        Ok(())
    }

    fn update(
        &mut self,
        index: usize,
        id: Id,
        base_seq: Seq,
        block_type: Option<BlockType>,
        props: Option<&orch_api_contract::documents::BlockProps>,
        content: Option<&BlockContent>,
    ) -> Result<(), Refusal> {
        let path = format!("ops[{index}]");
        let Some(node) = self.tree.live(id) else {
            return Err(invalid(
                format!("{path}.id"),
                "names a block that was removed earlier in this batch",
            ));
        };
        if node.seq_updated != base_seq && node.seq_updated != self.seq {
            return Err(Refusal::Stale(vec![id]));
        }
        let old = node.checked.clone().ok_or(Refusal::NotLoaded(id))?;
        let parent_type = node
            .parent
            .and_then(|parent| self.tree.get(parent))
            .map(|parent| parent.block_type);
        let new_type = block_type.unwrap_or(node.block_type);
        if new_type != node.block_type {
            if let Err(why) = blocks::may_hold(parent_type, new_type) {
                return Err(invalid(format!("{path}.type"), why));
            }
            for child in self.tree.kids(Some(id)) {
                let child_type = self.tree.get(*child).map(|child| child.block_type);
                if let Some(child_type) = child_type
                    && let Err(why) = blocks::may_hold(Some(new_type), child_type)
                {
                    return Err(invalid(format!("{path}.type"), why));
                }
            }
            if new_type == BlockType::Columns {
                self.columns.insert(id);
            }
        }
        let old_props = blocks::props_from_json(&old.props);
        let props = props.unwrap_or(&old_props);
        let content = content.unwrap_or(&old.content);
        let checked = match blocks::check(new_type, Some(props), Some(content)) {
            Ok(checked) => checked,
            Err(faults) => {
                return Err(Refusal::Invalid(
                    faults
                        .into_iter()
                        .map(|fault| Fault::at(format!("{path}.{}", fault.path), fault.message))
                        .collect(),
                ));
            }
        };
        if !blocks::fits_a_block(checked.bytes) {
            return Err(Refusal::BlockTooLarge {
                path,
                bytes: checked.bytes,
            });
        }
        if checked == old {
            return Ok(());
        }
        if let Some(node) = self.tree.nodes.get_mut(&id) {
            node.block_type = new_type;
            node.checked = Some(checked);
        }
        self.stamp(id);
        if !self.made_set.contains(&id) && !self.updated.contains(&id) {
            self.updated.push(id);
        }
        Ok(())
    }

    fn move_block(
        &mut self,
        index: usize,
        id: Id,
        parent: Option<Id>,
        after: Option<Id>,
    ) -> Result<(), Refusal> {
        let path = format!("ops[{index}]");
        let Some(node) = self.tree.live(id) else {
            return Err(invalid(
                format!("{path}.id"),
                "names a block that was removed earlier in this batch",
            ));
        };
        let (old_parent, block_type) = (node.parent, node.block_type);
        let parent_type = match parent {
            None => None,
            Some(parent) => {
                if self.tree.is_within(parent, id) {
                    return Err(invalid(
                        format!("{path}.parent_id"),
                        "a block is not moved under itself",
                    ));
                }
                match self.tree.live(parent) {
                    Some(node) => Some(node.block_type),
                    None => {
                        return Err(invalid(
                            format!("{path}.parent_id"),
                            "was removed earlier in this batch",
                        ));
                    }
                }
            }
        };
        if let Err(why) = blocks::may_hold(parent_type, block_type) {
            return Err(invalid(format!("{path}.parent_id"), why));
        }
        let depth = parent.map_or(0, |parent| self.tree.depth_of(parent) + 1);
        if depth + self.tree.height_of(id) > blocks::nesting_limit() {
            return Err(Refusal::TooDeep {
                path: format!("{path}.parent_id"),
            });
        }
        let position = self.tree.place(parent, after, Some(id));
        self.tree.unlink(id);
        if let Some(node) = self.tree.nodes.get_mut(&id) {
            node.parent = parent;
            node.position = position;
        }
        self.tree.link(id);
        self.stamp(id);
        if !self.made_set.contains(&id) && !self.moved.contains(&id) {
            self.moved.push(id);
        }
        self.columns.extend(old_parent);
        self.columns.extend(parent);
        Ok(())
    }

    fn remove(&mut self, id: Id) {
        let Some(node) = self.tree.live(id) else {
            // What is gone, or was never here, stays as it is.
            return;
        };
        let parent = node.parent;
        let mut gone = self.tree.descendants(id);
        gone.push(id);
        self.tree.unlink(id);
        for each in gone {
            if let Some(node) = self.tree.nodes.get_mut(&each) {
                node.removed = true;
            }
            self.tree.children.remove(&Some(each));
            self.stamp(each);
            if !self.made_set.contains(&each) {
                self.removed.push(each);
            }
        }
        self.columns.extend(parent);
    }
}

/// Whether a batch a client sent is no larger than a batch may be. A document
/// that is made from a long text is made of more operations than that, and
/// is not a batch.
pub fn check_size(ops: &[DocumentOp]) -> Result<(), Refusal> {
    if ops.len() > usize::try_from(DOCUMENT_OPS_MAX).unwrap_or(usize::MAX) {
        return Err(invalid(
            "ops",
            format!("a batch holds at most {DOCUMENT_OPS_MAX} operations"),
        ));
    }
    Ok(())
}

/// Applies `ops` to `tree`, stamping what they touch with `seq`. The document
/// may hold `max_blocks` live blocks afterwards. On a refusal the tree is not
/// to be used again.
///
/// Every block that an operation builds on is first compared with the tree:
/// one that was removed, or changed after the stamp `base_seq` the batch was
/// written on, makes the batch [`Refusal::Stale`] with all of them named.
pub fn apply(
    tree: &mut Tree,
    ops: &[DocumentOp],
    seq: Seq,
    max_blocks: i64,
) -> Result<Effects, Refusal> {
    let wanted = references(ops);
    let made: HashSet<Id> = wanted.made.iter().copied().collect();
    let live_before: HashSet<Id> = tree
        .nodes
        .values()
        .filter(|node| !node.removed)
        .map(|node| node.id)
        .collect();

    // What the batch builds on, compared with what is stored.
    let mut stale: Vec<Id> = Vec::new();
    let mut builds_on = |index: usize, member: &str, id: Id, base: Option<Seq>| match tree.get(id) {
        None if made.contains(&id) => Ok(()),
        None => Err(invalid(
            format!("ops[{index}].{member}"),
            "names no block of this document",
        )),
        Some(node) => {
            if (node.removed || base.is_some_and(|base| node.seq_updated != base))
                && !stale.contains(&id)
            {
                stale.push(id);
            }
            Ok(())
        }
    };
    for (index, op) in ops.iter().enumerate() {
        match op {
            DocumentOp::Insert { parent_id, .. } => {
                if let Some(parent) = parent_id {
                    builds_on(index, "parent_id", *parent, None)?;
                }
            }
            DocumentOp::Update { id, base_seq, .. } => {
                builds_on(index, "id", *id, Some(*base_seq))?;
            }
            DocumentOp::Move { id, parent_id, .. } => {
                builds_on(index, "id", *id, None)?;
                if let Some(parent) = parent_id {
                    builds_on(index, "parent_id", *parent, None)?;
                }
            }
            DocumentOp::Remove { .. } => {}
        }
    }
    if !stale.is_empty() {
        return Err(Refusal::Stale(stale));
    }

    let mut run = Run {
        tree,
        seq,
        made: Vec::new(),
        made_set: HashSet::new(),
        updated: Vec::new(),
        moved: Vec::new(),
        removed: Vec::new(),
        columns: HashSet::new(),
    };
    for (index, op) in ops.iter().enumerate() {
        match op {
            DocumentOp::Insert {
                block,
                parent_id,
                after_id,
            } => run.insert(index, block, *parent_id, *after_id)?,
            DocumentOp::Update {
                id,
                base_seq,
                block_type,
                props,
                content,
            } => run.update(
                index,
                *id,
                *base_seq,
                *block_type,
                props.as_ref(),
                content.as_ref(),
            )?,
            DocumentOp::Move {
                id,
                parent_id,
                after_id,
            } => run.move_block(index, *id, *parent_id, *after_id)?,
            DocumentOp::Remove { id } => run.remove(*id),
        }
    }

    for container in &run.columns {
        let Some(node) = run.tree.live(*container) else {
            continue;
        };
        if node.block_type == BlockType::Columns
            && !blocks::columns_fit(run.tree.kids(Some(*container)).len())
        {
            return Err(invalid("ops", "a columns block holds two to six columns"));
        }
    }
    let live = i64::try_from(run.tree.live_count()).unwrap_or(i64::MAX);
    if live > max_blocks {
        return Err(Refusal::TooManyBlocks { limit: max_blocks });
    }

    let written = |tree: &Tree, id: Id| -> Option<Written> {
        let node = tree.live(id)?;
        Some(Written {
            id,
            parent: node.parent,
            position: node.position.clone(),
            checked: node.checked.clone()?,
        })
    };
    let inserted: Vec<Written> = run
        .made
        .iter()
        .filter_map(|id| written(run.tree, *id))
        .collect();
    let removed: Vec<Id> = run
        .removed
        .iter()
        .copied()
        .filter(|id| live_before.contains(id))
        .collect();
    let updated: Vec<Written> = run
        .updated
        .iter()
        .filter(|id| !removed.contains(id))
        .filter_map(|id| written(run.tree, *id))
        .collect();
    let moved: Vec<Moved> = run
        .moved
        .iter()
        .filter(|id| !removed.contains(id))
        .filter_map(|id| {
            run.tree.live(*id).map(|node| Moved {
                id: *id,
                parent: node.parent,
                position: node.position.clone(),
            })
        })
        .collect();
    let mut touched: Vec<Id> = Vec::new();
    for id in inserted
        .iter()
        .map(|each| each.id)
        .chain(updated.iter().map(|each| each.id))
        .chain(moved.iter().map(|each| each.id))
        .chain(removed.iter().copied())
    {
        if !touched.contains(&id) {
            touched.push(id);
        }
    }
    let mut named: Vec<Id> = Vec::new();
    let mut files: Vec<(Id, Id)> = Vec::new();
    for each in inserted.iter().chain(updated.iter()) {
        for user in mentions::in_content(&each.checked.content) {
            if !named.contains(&user) {
                named.push(user);
            }
        }
        for file in each.checked.file_ids() {
            files.push((each.id, file));
        }
    }
    let replaces_everything =
        !live_before.is_empty() && live_before.iter().all(|id| run.tree.live(*id).is_none());
    let count = |list: usize| i64::try_from(list).unwrap_or(i64::MAX);
    Ok(Effects {
        tally: Tally {
            inserted: count(inserted.len()),
            updated: count(updated.len()),
            moved: count(moved.len()),
            removed: count(removed.len()),
        },
        inserted,
        updated,
        moved,
        removed,
        touched,
        replaces_everything,
        named,
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_api_contract::documents::{BlockProps, DOCUMENT_BLOCK_MAX_BYTES, Inline};
    use orch_types::new_id;
    use serde_json::json;

    fn text(words: &str) -> Inline {
        Inline::Text {
            text: words.to_owned(),
            marks: None,
            href: None,
            color: None,
            background: None,
        }
    }

    fn paragraph(id: Id, words: &str) -> NewBlock {
        NewBlock {
            id,
            block_type: BlockType::Paragraph,
            props: None,
            content: Some(BlockContent::Inline(vec![text(words)])),
            children: None,
        }
    }

    fn insert(block: NewBlock, parent: Option<Id>, after: Option<Id>) -> DocumentOp {
        DocumentOp::Insert {
            block,
            parent_id: parent,
            after_id: after,
        }
    }

    /// A document of these paragraphs at the top, stamped 1, read in full.
    fn tree_of(paragraphs: &[(Id, &str)]) -> Tree {
        let mut ops = Vec::new();
        let mut after = None;
        for (id, words) in paragraphs {
            ops.push(insert(paragraph(*id, words), None, after));
            after = Some(*id);
        }
        let mut tree = Tree::default();
        apply(&mut tree, &ops, 1, 5_000).unwrap();
        tree
    }

    fn words_of(tree: &Tree, id: Id) -> String {
        tree.get(id)
            .and_then(|node| node.checked.as_ref())
            .map(|checked| checked.text.clone())
            .unwrap()
    }

    fn order(tree: &Tree, parent: Option<Id>) -> Vec<Id> {
        tree.kids(parent).to_vec()
    }

    fn update_text(id: Id, base_seq: Seq, words: &str) -> DocumentOp {
        DocumentOp::Update {
            id,
            base_seq,
            block_type: None,
            props: None,
            content: Some(BlockContent::Inline(vec![text(words)])),
        }
    }

    #[test]
    fn released_content_can_be_read_again_without_losing_order_or_conflict_checks() {
        let (a, b) = (new_id(), new_id());
        let mut tree = tree_of(&[(a, "first"), (b, "second")]);
        let saved = tree.get(a).unwrap().clone();
        tree.discard_data(&[a, b]);
        assert!(tree.nodes.values().all(|node| node.checked.is_none()));
        assert_eq!(order(&tree, None), vec![a, b]);
        assert_eq!(tree.get(a).unwrap().position, saved.position);
        assert_eq!(tree.get(a).unwrap().seq_updated, 1);
        let refused = apply(&mut tree, &[update_text(a, 1, "changed")], 2, 5_000).unwrap_err();
        assert_eq!(refused, Refusal::NotLoaded(a));
        tree.supply(saved);
        let stale = apply(&mut tree, &[update_text(a, 0, "changed")], 2, 5_000).unwrap_err();
        assert_eq!(stale, Refusal::Stale(vec![a]));
        let changed = apply(&mut tree, &[update_text(a, 1, "changed")], 2, 5_000).unwrap();
        assert_eq!(changed.updated.len(), 1);
        assert_eq!(words_of(&tree, a), "changed");
        assert!(tree.get(b).unwrap().checked.is_none());
        assert_eq!(order(&tree, None), vec![a, b]);
        tree.discard_data(&[a]);
        assert!(tree.nodes.values().all(|node| node.checked.is_none()));
        assert_eq!(tree.get(a).unwrap().seq_updated, 2);
    }

    #[test]
    fn blocks_are_put_where_the_batch_says_and_the_children_come_with_them() {
        let (a, b, c, kid) = (new_id(), new_id(), new_id(), new_id());
        let mut tree = tree_of(&[(a, "a"), (b, "b")]);
        let toggle = NewBlock {
            id: c,
            block_type: BlockType::Toggle,
            props: None,
            content: Some(BlockContent::Inline(vec![text("more")])),
            children: Some(vec![paragraph(kid, "inside")]),
        };
        let effects = apply(&mut tree, &[insert(toggle, None, Some(a))], 2, 5_000).unwrap();
        assert_eq!(
            order(&tree, None),
            [a, c, b],
            "after the block it was put after"
        );
        assert_eq!(order(&tree, Some(c)), [kid]);
        assert_eq!(effects.tally.inserted, 2);
        assert_eq!(
            effects.inserted.iter().map(|w| w.id).collect::<Vec<_>>(),
            [c, kid],
            "a parent before what is under it"
        );
        assert_eq!(tree.get(c).unwrap().seq_updated, 2);
        // First among its siblings.
        let first = new_id();
        apply(
            &mut tree,
            &[insert(paragraph(first, "first"), None, None)],
            3,
            5_000,
        )
        .unwrap();
        assert_eq!(order(&tree, None), [first, a, c, b]);
    }

    #[test]
    fn a_block_put_after_one_that_is_gone_goes_to_the_end() {
        let (a, b, new) = (new_id(), new_id(), new_id());
        let mut tree = tree_of(&[(a, "a"), (b, "b")]);
        let gone = new_id();
        apply(
            &mut tree,
            &[insert(paragraph(new, "n"), None, Some(gone))],
            2,
            5_000,
        )
        .unwrap();
        assert_eq!(order(&tree, None), [a, b, new]);
        // After a block that is no child of that parent.
        let other = new_id();
        let kid = new_id();
        apply(
            &mut tree,
            &[
                insert(paragraph(other, "o"), None, None),
                insert(paragraph(kid, "k"), Some(other), Some(a)),
            ],
            3,
            5_000,
        )
        .unwrap();
        assert_eq!(order(&tree, Some(other)), [kid]);
    }

    #[test]
    fn a_batch_that_fails_halfway_leaves_nothing_behind_for_the_caller_to_store() {
        let (a, b) = (new_id(), new_id());
        let mut tree = tree_of(&[(a, "a")]);
        let bad = DocumentOp::Update {
            id: a,
            base_seq: 1,
            block_type: None,
            props: Some(serde_json::from_value::<BlockProps>(json!({ "level": 9 })).unwrap()),
            content: None,
        };
        let refusal = apply(
            &mut tree,
            &[insert(paragraph(b, "fine"), None, None), bad],
            2,
            5_000,
        )
        .unwrap_err();
        assert!(
            matches!(&refusal, Refusal::Invalid(faults) if faults[0].path.starts_with("ops[1]"))
        );
    }

    #[test]
    fn a_change_written_on_an_older_state_makes_the_whole_batch_stale_and_names_every_block_missed()
    {
        let (a, b, c) = (new_id(), new_id(), new_id());
        let mut tree = tree_of(&[(a, "a"), (b, "b"), (c, "c")]);
        // Somebody else changed b and a at sequence 2 and 3.
        apply(&mut tree, &[update_text(b, 1, "b2")], 2, 5_000).unwrap();
        apply(&mut tree, &[update_text(a, 1, "a2")], 3, 5_000).unwrap();
        let mine = [
            update_text(c, 1, "c mine"),
            update_text(b, 1, "b mine"),
            update_text(a, 1, "a mine"),
        ];
        let refusal = apply(&mut tree, &mine, 4, 5_000).unwrap_err();
        assert_eq!(
            refusal,
            Refusal::Stale(vec![b, a]),
            "c was not touched by anybody"
        );
    }

    #[test]
    fn two_people_who_write_in_different_blocks_both_succeed_in_turn() {
        let (a, b) = (new_id(), new_id());
        let mut tree = tree_of(&[(a, "a"), (b, "b")]);
        apply(&mut tree, &[update_text(a, 1, "by ada")], 2, 5_000).unwrap();
        // Bo wrote on b at sequence 1 and a was changed meanwhile: no conflict.
        apply(&mut tree, &[update_text(b, 1, "by bo")], 3, 5_000).unwrap();
        assert_eq!(words_of(&tree, a), "by ada");
        assert_eq!(words_of(&tree, b), "by bo");
    }

    #[test]
    fn a_block_removed_since_is_stale_and_so_is_a_parent_that_is_gone() {
        let (a, b) = (new_id(), new_id());
        let mut tree = tree_of(&[(a, "a")]);
        // b stands in the tree as a removed row, as the caller loads one that an operation names.
        let mut tombstone = tree.get(a).unwrap().clone();
        tombstone.id = b;
        tombstone.removed = true;
        tree.nodes.insert(b, tombstone);
        let refusal = apply(&mut tree, &[update_text(b, 1, "x")], 2, 5_000).unwrap_err();
        assert_eq!(refusal, Refusal::Stale(vec![b]));
        let refusal = apply(
            &mut tree,
            &[insert(paragraph(new_id(), "kid"), Some(b), None)],
            2,
            5_000,
        )
        .unwrap_err();
        assert_eq!(refusal, Refusal::Stale(vec![b]));
        let refusal = apply(
            &mut tree,
            &[DocumentOp::Move {
                id: a,
                parent_id: Some(b),
                after_id: None,
            }],
            2,
            5_000,
        )
        .unwrap_err();
        assert_eq!(refusal, Refusal::Stale(vec![b]));
    }

    #[test]
    fn a_block_that_was_never_there_is_a_fault_of_the_request_and_no_conflict() {
        let a = new_id();
        let mut tree = tree_of(&[(a, "a")]);
        let refusal = apply(&mut tree, &[update_text(new_id(), 1, "x")], 2, 5_000).unwrap_err();
        assert!(matches!(refusal, Refusal::Invalid(faults) if faults[0].path == "ops[0].id"));
    }

    #[test]
    fn a_block_made_earlier_in_the_batch_may_be_changed_later_in_it_without_knowing_its_stamp() {
        let (a, b) = (new_id(), new_id());
        let mut tree = tree_of(&[(a, "a")]);
        let effects = apply(
            &mut tree,
            &[
                insert(paragraph(b, "draft"), None, Some(a)),
                update_text(b, 0, "final"),
            ],
            2,
            5_000,
        )
        .unwrap();
        assert_eq!(words_of(&tree, b), "final");
        assert_eq!(effects.tally.inserted, 1);
        assert_eq!(
            effects.tally.updated, 0,
            "it is one new block, not a change"
        );
        assert_eq!(effects.inserted[0].checked.text, "final");
    }

    #[test]
    fn an_identifier_in_use_is_refused_and_so_is_one_used_twice_in_the_batch() {
        let a = new_id();
        let mut tree = tree_of(&[(a, "a")]);
        let refusal = apply(
            &mut tree,
            &[insert(paragraph(a, "again"), None, None)],
            2,
            5_000,
        )
        .unwrap_err();
        assert_eq!(refusal, Refusal::Exists(a));
        let twice = new_id();
        let refusal = apply(
            &mut tree,
            &[
                insert(paragraph(twice, "one"), None, None),
                insert(paragraph(twice, "two"), None, None),
            ],
            2,
            5_000,
        )
        .unwrap_err();
        assert_eq!(refusal, Refusal::Exists(twice));
    }

    #[test]
    fn removing_takes_everything_under_a_block_and_removing_what_is_gone_changes_nothing() {
        let (a, kid, grandkid) = (new_id(), new_id(), new_id());
        let mut tree = tree_of(&[(a, "a")]);
        apply(
            &mut tree,
            &[
                insert(paragraph(kid, "k"), Some(a), None),
                insert(paragraph(grandkid, "g"), Some(kid), None),
            ],
            2,
            5_000,
        )
        .unwrap();
        let effects = apply(&mut tree, &[DocumentOp::Remove { id: a }], 3, 5_000).unwrap();
        assert_eq!(effects.tally.removed, 3);
        assert_eq!(tree.live_count(), 0);
        assert!(effects.replaces_everything);
        let again = apply(
            &mut tree,
            &[
                DocumentOp::Remove { id: a },
                DocumentOp::Remove { id: new_id() },
            ],
            4,
            5_000,
        )
        .unwrap();
        assert!(again.touched.is_empty() && again.tally.removed == 0);
    }

    #[test]
    fn a_block_made_and_removed_in_one_batch_was_never_there() {
        let (a, b) = (new_id(), new_id());
        let mut tree = tree_of(&[(a, "a")]);
        let effects = apply(
            &mut tree,
            &[
                insert(paragraph(b, "x"), None, None),
                DocumentOp::Remove { id: b },
            ],
            2,
            5_000,
        )
        .unwrap();
        assert!(effects.inserted.is_empty() && effects.removed.is_empty());
        assert!(effects.touched.is_empty());
        assert!(!effects.replaces_everything);
    }

    #[test]
    fn a_batch_replaces_everything_when_no_block_that_was_live_before_is_left() {
        let (a, b) = (new_id(), new_id());
        let mut tree = tree_of(&[(a, "a"), (b, "b")]);
        let fresh = new_id();
        let partly = apply(
            &mut tree,
            &[
                DocumentOp::Remove { id: a },
                insert(paragraph(fresh, "new"), None, None),
            ],
            2,
            5_000,
        )
        .unwrap();
        assert!(!partly.replaces_everything, "b is still there");
        let wholly = apply(
            &mut tree,
            &[
                DocumentOp::Remove { id: b },
                DocumentOp::Remove { id: fresh },
            ],
            3,
            5_000,
        )
        .unwrap();
        assert!(wholly.replaces_everything);
        // A document with nothing in it is not replaced by anything.
        let again = apply(
            &mut tree,
            &[insert(paragraph(new_id(), "x"), None, None)],
            4,
            5_000,
        )
        .unwrap();
        assert!(!again.replaces_everything);
    }

    #[test]
    fn moving_puts_a_block_with_what_is_under_it_somewhere_else_and_never_under_itself() {
        let (a, b, kid) = (new_id(), new_id(), new_id());
        let mut tree = tree_of(&[(a, "a"), (b, "b")]);
        apply(
            &mut tree,
            &[insert(paragraph(kid, "k"), Some(a), None)],
            2,
            5_000,
        )
        .unwrap();
        let effects = apply(
            &mut tree,
            &[DocumentOp::Move {
                id: a,
                parent_id: Some(b),
                after_id: None,
            }],
            3,
            5_000,
        )
        .unwrap();
        assert_eq!(order(&tree, None), [b]);
        assert_eq!(order(&tree, Some(b)), [a]);
        assert_eq!(
            order(&tree, Some(a)),
            [kid],
            "what is under it goes with it"
        );
        assert_eq!(effects.tally.moved, 1);
        for parent in [Some(a), Some(kid)] {
            let refusal = apply(
                &mut tree,
                &[DocumentOp::Move {
                    id: b,
                    parent_id: parent,
                    after_id: None,
                }],
                4,
                5_000,
            )
            .unwrap_err();
            assert!(
                matches!(refusal, Refusal::Invalid(faults) if faults[0].path == "ops[0].parent_id"),
                "{parent:?} is under b"
            );
        }
        let refusal = apply(
            &mut tree,
            &[DocumentOp::Move {
                id: b,
                parent_id: Some(b),
                after_id: None,
            }],
            4,
            5_000,
        )
        .unwrap_err();
        assert!(matches!(refusal, Refusal::Invalid(_)));
    }

    #[test]
    fn a_block_goes_only_where_its_type_may_stand() {
        let (code, p, columns, c1, c2) = (new_id(), new_id(), new_id(), new_id(), new_id());
        let mut tree = Tree::default();
        let code_block = NewBlock {
            id: code,
            block_type: BlockType::Code,
            props: Some(serde_json::from_value(json!({ "language": "rust" })).unwrap()),
            content: None,
            children: None,
        };
        apply(
            &mut tree,
            &[
                insert(code_block, None, None),
                insert(paragraph(p, "p"), None, Some(code)),
            ],
            1,
            5_000,
        )
        .unwrap();
        let refusal = apply(
            &mut tree,
            &[insert(paragraph(new_id(), "k"), Some(code), None)],
            2,
            5_000,
        )
        .unwrap_err();
        assert!(
            matches!(refusal, Refusal::Invalid(faults) if faults[0].path == "ops[0].block.type")
        );
        let column = |id| NewBlock {
            id,
            block_type: BlockType::Column,
            props: None,
            content: None,
            children: Some(vec![]),
        };
        let columns_block = |children| NewBlock {
            id: columns,
            block_type: BlockType::Columns,
            props: None,
            content: None,
            children: Some(children),
        };
        // A refused batch leaves its tree unfit for another, so each try is on a copy.
        let one = apply(
            &mut tree.clone(),
            &[insert(columns_block(vec![column(c1)]), None, None)],
            2,
            5_000,
        )
        .unwrap_err();
        assert!(matches!(one, Refusal::Invalid(_)), "one column is too few");
        let seven: Vec<NewBlock> = (0..7).map(|_| column(new_id())).collect();
        let many = apply(
            &mut tree.clone(),
            &[insert(columns_block(seven), None, None)],
            2,
            5_000,
        )
        .unwrap_err();
        assert!(matches!(many, Refusal::Invalid(_)), "seven are too many");
        apply(
            &mut tree,
            &[insert(
                columns_block(vec![column(c1), column(c2)]),
                None,
                None,
            )],
            2,
            5_000,
        )
        .unwrap();
        // Taking one of two columns away leaves too few.
        let refusal = apply(
            &mut tree.clone(),
            &[DocumentOp::Remove { id: c2 }],
            3,
            5_000,
        )
        .unwrap_err();
        assert!(matches!(refusal, Refusal::Invalid(_)));
        // A paragraph does not go among the columns, and a column does not go among paragraphs.
        let refusal = apply(
            &mut tree.clone(),
            &[DocumentOp::Move {
                id: p,
                parent_id: Some(columns),
                after_id: None,
            }],
            3,
            5_000,
        )
        .unwrap_err();
        assert!(matches!(refusal, Refusal::Invalid(_)));
        let refusal = apply(
            &mut tree.clone(),
            &[DocumentOp::Move {
                id: c1,
                parent_id: None,
                after_id: None,
            }],
            3,
            5_000,
        )
        .unwrap_err();
        assert!(matches!(refusal, Refusal::Invalid(_)));
    }

    #[test]
    fn blocks_nest_at_most_sixteen_deep() {
        let mut tree = Tree::default();
        let mut ids = vec![new_id()];
        let mut ops = vec![insert(paragraph(ids[0], "0"), None, None)];
        for depth in 1..=16 {
            let id = new_id();
            ops.push(insert(paragraph(id, "n"), Some(ids[depth - 1]), None));
            ids.push(id);
        }
        apply(&mut tree, &ops, 1, 5_000).unwrap();
        assert_eq!(tree.depth_of(ids[16]), 16);
        let refusal = apply(
            &mut tree,
            &[insert(paragraph(new_id(), "too deep"), Some(ids[16]), None)],
            2,
            5_000,
        )
        .unwrap_err();
        assert!(matches!(refusal, Refusal::TooDeep { path } if path == "ops[0].block"));
        // Moving a block whose own children would end up too deep is refused as well.
        let loose = new_id();
        let loose_kid = new_id();
        apply(
            &mut tree,
            &[
                insert(paragraph(loose, "loose"), None, None),
                insert(paragraph(loose_kid, "kid"), Some(loose), None),
            ],
            2,
            5_000,
        )
        .unwrap();
        let refusal = apply(
            &mut tree,
            &[DocumentOp::Move {
                id: loose,
                parent_id: Some(ids[15]),
                after_id: None,
            }],
            3,
            5_000,
        )
        .unwrap_err();
        assert!(matches!(refusal, Refusal::TooDeep { .. }));
    }

    #[test]
    fn the_limits_of_a_block_and_of_a_document_are_kept() {
        let mut tree = Tree::default();
        let big = "x".repeat(usize::try_from(DOCUMENT_BLOCK_MAX_BYTES).unwrap());
        let refusal = apply(
            &mut tree,
            &[insert(paragraph(new_id(), &big), None, None)],
            1,
            5_000,
        )
        .unwrap_err();
        assert!(matches!(refusal, Refusal::BlockTooLarge { .. }));
        let few: Vec<DocumentOp> = (0..3)
            .map(|n| insert(paragraph(new_id(), &n.to_string()), None, None))
            .collect();
        let refusal = apply(&mut tree, &few, 1, 2).unwrap_err();
        assert_eq!(refusal, Refusal::TooManyBlocks { limit: 2 });
        let many: Vec<DocumentOp> = (0..=DOCUMENT_OPS_MAX)
            .map(|n| insert(paragraph(new_id(), &n.to_string()), None, None))
            .collect();
        let refusal = check_size(&many).unwrap_err();
        assert!(matches!(refusal, Refusal::Invalid(faults) if faults[0].path == "ops"));
        assert!(check_size(&many[..usize::try_from(DOCUMENT_OPS_MAX).unwrap()]).is_ok());
    }

    #[test]
    fn a_change_that_changes_nothing_is_not_a_change() {
        let a = new_id();
        let mut tree = tree_of(&[(a, "same")]);
        let effects = apply(&mut tree, &[update_text(a, 1, "same")], 2, 5_000).unwrap();
        assert!(effects.touched.is_empty());
        assert_eq!(tree.get(a).unwrap().seq_updated, 1);
    }

    #[test]
    fn turning_a_block_into_another_type_is_checked_against_where_it_stands_and_what_it_holds() {
        let (list, item, code) = (new_id(), new_id(), new_id());
        let mut tree = Tree::default();
        apply(
            &mut tree,
            &[
                insert(paragraph(list, "list"), None, None),
                insert(paragraph(item, "item"), Some(list), None),
                insert(paragraph(code, "x"), None, Some(list)),
            ],
            1,
            5_000,
        )
        .unwrap();
        // A paragraph that holds a block cannot become a code block.
        let into_code = |id: Id| DocumentOp::Update {
            id,
            base_seq: 1,
            block_type: Some(BlockType::Code),
            props: Some(serde_json::from_value(json!({ "language": "rust" })).unwrap()),
            content: None,
        };
        let refusal = apply(&mut tree, &[into_code(list)], 2, 5_000).unwrap_err();
        assert!(matches!(refusal, Refusal::Invalid(faults) if faults[0].path == "ops[0].type"));
        // One that holds nothing can, and loses nothing it must keep.
        apply(&mut tree, &[into_code(code)], 2, 5_000).unwrap();
        assert_eq!(tree.get(code).unwrap().block_type, BlockType::Code);
        // Turned into a heading without saying its level, it is refused: the old props do not fit.
        let into_heading = DocumentOp::Update {
            id: item,
            base_seq: 1,
            block_type: Some(BlockType::Heading),
            props: None,
            content: None,
        };
        let refusal = apply(&mut tree, &[into_heading], 3, 5_000).unwrap_err();
        assert!(matches!(refusal, Refusal::Invalid(_)));
    }

    #[test]
    fn the_people_a_batch_names_are_read_from_what_it_writes() {
        let (a, ada, bo) = (new_id(), new_id(), new_id());
        let mut tree = tree_of(&[(a, "a")]);
        let mention = |user| Inline::Mention {
            user_id: user,
            user: None,
        };
        let with_names = NewBlock {
            id: new_id(),
            block_type: BlockType::Paragraph,
            props: None,
            content: Some(BlockContent::Inline(vec![
                mention(ada),
                text(" and "),
                mention(bo),
                mention(ada),
            ])),
            children: None,
        };
        let effects = apply(&mut tree, &[insert(with_names, None, None)], 2, 5_000).unwrap();
        assert_eq!(effects.named, [ada, bo]);
    }
}
