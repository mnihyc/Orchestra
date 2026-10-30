//! Changing a document to what its Markdown now says while keeping what each block already is: its identifier, its comments' anchors, its links, and whatever Markdown has no form for.
//!
//! An agent that rewrites a document, or a passage of one, hands over Markdown, and Markdown makes new blocks. A
//! change that gave every block a new identifier would cut every comment off its words and break every link
//! to a block. So the new blocks are laid against the old ones, level by level: a block that the Markdown
//! writes as the old one did is the old one, left alone with its colours, its columns and what else it has; a
//! block that is written differently but plainly the same block (the same kind, the words mostly the same,
//! or the only one of its kind in its place) is the old one with its text changed; the rest are removed and
//! inserted. The blocks under a block are laid the same way. [`plan`] gives the operations that do it; it
//! touches nothing, so that whoever applies them can first say what they would change.

use std::collections::{HashMap, HashSet};

use orch_api_contract::documents::{
    BlockContent, BlockProps, BlockType, DocumentBlock, DocumentOp, NewBlock,
};
use orch_types::{Id, Json, Seq, Time};

use crate::blocks::{self, Lookup};
use crate::markdown::{self, RenderOptions};

/// How many cells a table of positions may have before the exact alignment of a stretch gives way to the one that
/// looks only at what is unique: a stretch of a thousand blocks on each side is a million cells, and the exact
/// alignment of it is the most that is worth its time and memory.
const ALIGN_CELLS_MAX: usize = 1_000_000;

/// How many pairs of an old and a new block one stretch of a level may compare for likeness before the blocks of the
/// stretch are paired in step instead: two stretches of three hundred blocks each are the most that are compared
/// each with each. A document changed everywhere is thereby laid against its old blocks in the time of its length.
const PAIR_CELLS_MAX: usize = 100_000;

/// How many such pairs one plan compares in all, over every stretch of every level: a document changed everywhere is
/// laid against its old blocks in the time of its length, whatever its shape.
const PAIRS_TOTAL_MAX: usize = 500_000;

/// The most characters of the words of a block that are compared for likeness.
const COMPARED_CHARS_MAX: usize = 400;

/// How alike the words of two blocks of different kinds must be for the one to become the other: the share of
/// their pairs of letters that they have in common.
const SIMILAR_AT: f64 = 0.5;

/// A block as it is now, with the blocks under it.
#[derive(Debug, Clone)]
pub struct Existing {
    /// Its identifier.
    pub id: Id,
    /// What it is.
    pub block_type: BlockType,
    /// Its props as stored.
    pub props: Json,
    /// What it holds.
    pub content: BlockContent,
    /// The document sequence at which it last changed.
    pub seq_updated: Seq,
    /// The blocks under it, in order.
    pub children: Vec<Existing>,
}

/// What a plan does.
#[derive(Debug, Clone, Default)]
pub struct Merge {
    /// The operations, in the order they are applied: changes, removals, then insertions in the order of the page.
    pub ops: Vec<DocumentOp>,
    /// How many blocks the Markdown writes as they are, and which were left alone.
    pub kept: i64,
    /// The blocks that were given other words.
    pub updated: Vec<Id>,
    /// The blocks that were taken away, with what was under each.
    pub removed: Vec<Id>,
    /// How many blocks were put in, with what is under each.
    pub inserted: i64,
}

impl Merge {
    /// Whether nothing changes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// What a block says in Markdown on its own, without what is under it: two blocks that say the same are the same
/// block for a change made by Markdown.
fn say(
    lookup: &dyn Lookup,
    files_dir: Option<&str>,
    block_type: BlockType,
    props: &Json,
    content: &BlockContent,
) -> String {
    match block_type {
        BlockType::Columns => return "columns".to_owned(),
        BlockType::Column => {
            let share = props.get("share").and_then(Json::as_f64);
            return format!("column {share:?}");
        }
        _ => {}
    }
    let alone = DocumentBlock {
        id: Id::nil(),
        document_id: Id::nil(),
        parent_id: None,
        position: "1".to_owned(),
        block_type,
        props: blocks::props_from_json(props),
        content: content.clone(),
        removed: false,
        seq_created: 0,
        seq_updated: 0,
        updated_by: None,
        updated_at: Time::new(chrono::DateTime::UNIX_EPOCH),
    };
    let text = markdown::render(
        std::slice::from_ref(&alone),
        lookup,
        &RenderOptions {
            discussions: None,
            files_dir,
            preserve_reference_labels: false,
        },
    )
    .text;
    format!("{block_type:?}\u{0}{text}")
}

fn props_of(block: &NewBlock) -> Json {
    block.props.as_ref().map_or_else(
        || Json::Object(serde_json::Map::new()),
        |props| {
            serde_json::to_value(props).unwrap_or_else(|_| Json::Object(serde_json::Map::new()))
        },
    )
}

fn content_of(block: &NewBlock) -> BlockContent {
    block
        .content
        .clone()
        .unwrap_or_else(|| BlockContent::Inline(Vec::new()))
}

/// The words of a block, for telling whether two blocks are the same one.
fn words_of(block_type: BlockType, props: &Json, content: &BlockContent) -> String {
    blocks::plain_text(block_type, props, content)
}

/// How many characters two texts begin and end alike, together.
fn affix(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    prefix + suffix
}

fn bigrams(text: &str) -> HashSet<(char, char)> {
    let chars: Vec<char> = text.chars().collect();
    chars.windows(2).map(|pair| (pair[0], pair[1])).collect()
}

/// Whether two texts are plainly the same words changed a little.
fn similar(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let longest = a.chars().count().max(b.chars().count());
    if affix(a, b) * 2 >= longest {
        return true;
    }
    let (left, right) = (bigrams(a), bigrams(b));
    if left.is_empty() || right.is_empty() {
        return false;
    }
    let shared = left.intersection(&right).count() as f64;
    (2.0 * shared) / ((left.len() + right.len()) as f64) >= SIMILAR_AT
}

/// The kinds of block that are a line of words, and that one may be made into another without losing what it says.
fn is_text_kind(block_type: BlockType) -> bool {
    matches!(
        block_type,
        BlockType::Paragraph
            | BlockType::Heading
            | BlockType::Bullet
            | BlockType::Numbered
            | BlockType::Todo
            | BlockType::Toggle
            | BlockType::Quote
            | BlockType::Callout
    )
}

struct Side<'a> {
    old: &'a Existing,
    new: &'a NewBlock,
    old_words: &'a str,
    new_words: &'a str,
}

impl Side<'_> {
    /// Whether the new block may be the old one with other words, by what they are and hold, without looking at their
    /// words.
    fn may_be(&self) -> bool {
        let (old, new) = (self.old.block_type, self.new.block_type);
        if old != new && !(is_text_kind(old) && is_text_kind(new)) {
            return false;
        }
        // What cannot hold the blocks under the old one cannot take its place.
        self.old.children.is_empty() || blocks::may_have_children(new)
    }

    /// Whether the new block may be the old one with other words.
    fn pairable(&self, alone: bool) -> bool {
        if !self.may_be() {
            return false;
        }
        if self.old.block_type == self.new.block_type && alone {
            return true;
        }
        similar(self.old_words, self.new_words)
    }
}

/// The words of a block as they are compared: the first of them.
fn compared(words: String) -> String {
    let end = words
        .char_indices()
        .nth(COMPARED_CHARS_MAX)
        .map(|(end, _)| end);
    match end {
        Some(end) => words[..end].to_owned(),
        None => words,
    }
}

/// The pairs of positions, in order in both, at which a stretch of the old blocks and a stretch of the new ones say
/// exactly the same.
fn align(old: &[String], new: &[String]) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let (mut start, mut old_end, mut new_end) = (0, old.len(), new.len());
    while start < old_end && start < new_end && old[start] == new[start] {
        out.push((start, start));
        start += 1;
    }
    let mut tail: Vec<(usize, usize)> = Vec::new();
    while old_end > start && new_end > start && old[old_end - 1] == new[new_end - 1] {
        old_end -= 1;
        new_end -= 1;
        tail.push((old_end, new_end));
    }
    let (middle_old, middle_new) = (&old[start..old_end], &new[start..new_end]);
    if !middle_old.is_empty() && !middle_new.is_empty() {
        let found = if middle_old.len().saturating_mul(middle_new.len()) <= ALIGN_CELLS_MAX {
            longest_common(middle_old, middle_new)
        } else {
            unique_anchors(middle_old, middle_new)
        };
        out.extend(found.into_iter().map(|(a, b)| (a + start, b + start)));
    }
    tail.reverse();
    out.extend(tail);
    out
}

/// The longest run of the same blocks in the same order.
fn longest_common(old: &[String], new: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (old.len(), new.len());
    let mut table = vec![0_u32; (n + 1) * (m + 1)];
    let at = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[at(i, j)] = if old[i] == new[j] {
                table[at(i + 1, j + 1)] + 1
            } else {
                table[at(i + 1, j)].max(table[at(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if old[i] == new[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if table[at(i + 1, j)] >= table[at(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// For stretches too long to align exactly: the blocks that say something once on each side, in the longest order
/// they keep, and the stretches between them aligned again.
fn unique_anchors(old: &[String], new: &[String]) -> Vec<(usize, usize)> {
    let mut seen_old: HashMap<&str, (usize, usize)> = HashMap::new();
    for (index, key) in old.iter().enumerate() {
        let entry = seen_old.entry(key.as_str()).or_insert((index, 0));
        entry.1 += 1;
    }
    let mut seen_new: HashMap<&str, (usize, usize)> = HashMap::new();
    for (index, key) in new.iter().enumerate() {
        let entry = seen_new.entry(key.as_str()).or_insert((index, 0));
        entry.1 += 1;
    }
    let mut pairs: Vec<(usize, usize)> = seen_old
        .iter()
        .filter_map(|(key, (index, count))| {
            let (other, others) = seen_new.get(key)?;
            (*count == 1 && *others == 1).then_some((*index, *other))
        })
        .collect();
    pairs.sort_unstable();
    // The longest increasing run of the new positions, by patience.
    let mut tops: Vec<usize> = Vec::new();
    let mut back: Vec<Option<usize>> = vec![None; pairs.len()];
    for (at, (_, new_index)) in pairs.iter().enumerate() {
        let place = tops.partition_point(|top| pairs[*top].1 < *new_index);
        if place > 0 {
            back[at] = Some(tops[place - 1]);
        }
        if place == tops.len() {
            tops.push(at);
        } else {
            tops[place] = at;
        }
    }
    let mut chain: Vec<(usize, usize)> = Vec::new();
    let mut cursor = tops.last().copied();
    while let Some(at) = cursor {
        chain.push(pairs[at]);
        cursor = back[at];
    }
    chain.reverse();
    let (mut from_old, mut from_new) = (0, 0);
    let mut out = Vec::new();
    for (old_index, new_index) in chain
        .into_iter()
        .chain(std::iter::once((old.len(), new.len())))
    {
        let (gap_old, gap_new) = (&old[from_old..old_index], &new[from_new..new_index]);
        if gap_old.len().saturating_mul(gap_new.len()) <= ALIGN_CELLS_MAX
            || gap_old.is_empty()
            || gap_new.is_empty()
        {
            out.extend(
                align(gap_old, gap_new)
                    .into_iter()
                    .map(|(a, b)| (a + from_old, b + from_new)),
            );
        }
        if old_index < old.len() {
            out.push((old_index, new_index));
        }
        from_old = old_index + 1;
        from_new = new_index + 1;
    }
    out
}

/// What the plan is made with.
struct Planner<'a> {
    lookup: &'a dyn Lookup,
    files_dir: Option<&'a str>,
    merge: Merge,
    inserts: Vec<DocumentOp>,
    /// How many more pairs of blocks may be compared for likeness.
    pairs_left: usize,
}

impl Planner<'_> {
    fn say_old(&self, block: &Existing) -> String {
        say(
            self.lookup,
            self.files_dir,
            block.block_type,
            &block.props,
            &block.content,
        )
    }

    fn say_new(&self, block: &NewBlock) -> String {
        say(
            self.lookup,
            self.files_dir,
            block.block_type,
            &props_of(block),
            &content_of(block),
        )
    }

    /// The blocks under `parent`: what is there now laid against what is written.
    fn level(
        &mut self,
        old: &[Existing],
        new: Vec<NewBlock>,
        parent: Option<Id>,
        after: Option<Id>,
    ) {
        let old_keys: Vec<String> = old.iter().map(|block| self.say_old(block)).collect();
        let new_keys: Vec<String> = new.iter().map(|block| self.say_new(block)).collect();
        let kept = align(&old_keys, &new_keys);
        // Between the blocks that are kept: the old ones that may become the new ones.
        let mut old_of: Vec<Option<(usize, bool)>> = vec![None; new.len()];
        for (o, n) in &kept {
            old_of[*n] = Some((*o, true));
        }
        let mut gaps: Vec<((usize, usize), (usize, usize))> = Vec::new();
        let (mut from_old, mut from_new) = (0, 0);
        for (o, n) in kept
            .iter()
            .copied()
            .chain(std::iter::once((old.len(), new.len())))
        {
            gaps.push(((from_old, o), (from_new, n)));
            from_old = o + 1;
            from_new = n + 1;
        }
        for ((old_from, old_to), (new_from, new_to)) in gaps {
            let (old_count, new_count) = (old_to - old_from, new_to - new_from);
            if old_count == 0 || new_count == 0 {
                continue;
            }
            let alone = old_count == 1 && new_count == 1;
            let cells = old_count.saturating_mul(new_count);
            if cells > PAIR_CELLS_MAX.min(self.pairs_left) {
                // Too many to compare each with each: the blocks of the stretch are paired in step, kind with kind.
                for (o, n) in (old_from..old_to).zip(new_from..new_to) {
                    let side = Side {
                        old: &old[o],
                        new: &new[n],
                        old_words: "",
                        new_words: "",
                    };
                    if side.may_be() {
                        old_of[n] = Some((o, false));
                    }
                }
                continue;
            }
            self.pairs_left -= cells;
            let old_words: Vec<String> = old[old_from..old_to]
                .iter()
                .map(|block| compared(words_of(block.block_type, &block.props, &block.content)))
                .collect();
            let mut cursor = old_from;
            for n in new_from..new_to {
                let block = &new[n];
                let new_words = compared(words_of(
                    block.block_type,
                    &props_of(block),
                    &content_of(block),
                ));
                let found = (cursor..old_to).find(|o| {
                    Side {
                        old: &old[*o],
                        new: block,
                        old_words: &old_words[*o - old_from],
                        new_words: &new_words,
                    }
                    .pairable(alone)
                });
                if let Some(o) = found {
                    old_of[n] = Some((o, false));
                    cursor = o + 1;
                }
            }
        }
        let used: HashSet<usize> = old_of.iter().flatten().map(|(o, _)| *o).collect();
        for (o, block) in old.iter().enumerate() {
            if !used.contains(&o) {
                self.merge.ops.push(DocumentOp::Remove { id: block.id });
                self.merge.removed.push(block.id);
            }
        }
        let mut previous = after;
        for (n, block) in new.into_iter().enumerate() {
            match old_of[n] {
                Some((o, exact)) => {
                    let existing = &old[o];
                    if !exact {
                        self.update(existing, &block);
                    } else {
                        self.merge.kept += 1;
                    }
                    previous = Some(existing.id);
                    let children = block.children.unwrap_or_default();
                    self.level(&existing.children, children, Some(existing.id), None);
                }
                None => {
                    let id = block.id;
                    self.merge.inserted += 1;
                    self.inserts.push(DocumentOp::Insert {
                        block,
                        parent_id: parent,
                        after_id: previous,
                    });
                    previous = Some(id);
                }
            }
        }
    }

    /// The old block becomes the new one's words and kind; what the Markdown cannot say stays as it was.
    fn update(&mut self, old: &Existing, new: &NewBlock) {
        let mut props = match &old.props {
            Json::Object(map) => map.clone(),
            _ => serde_json::Map::new(),
        };
        if let Json::Object(written) = props_of(new) {
            for (name, value) in written {
                props.insert(name, value);
            }
        }
        let mut props = Json::Object(props);
        let content = content_of(new);
        // A kind that does not take what the old block had is given the new block's own props alone.
        let fits = blocks::check(
            new.block_type,
            Some(&blocks::props_from_json(&props)),
            Some(&content),
        )
        .is_ok();
        if !fits {
            props = props_of(new);
        }
        // Layout comments make a styled old block render differently from an otherwise identical plain update.
        // Once omitted properties are preserved, compare the actual stored shape before creating an edit.
        if old.block_type == new.block_type && props == old.props && content == old.content {
            self.merge.kept += 1;
            return;
        }
        let props: Option<BlockProps> = serde_json::from_value(props).ok();
        self.merge.ops.push(DocumentOp::Update {
            id: old.id,
            base_seq: old.seq_updated,
            block_type: (old.block_type != new.block_type).then_some(new.block_type),
            props,
            content: Some(content),
        });
        self.merge.updated.push(old.id);
    }
}

/// The operations that make the blocks under `parent` what `new` says, and no more than that: blocks the Markdown
/// writes as they are stand untouched. `after` is the block the first of them goes after (none: at the start).
/// `lookup` and `files_dir` are how the blocks are written to be compared: the same that wrote the Markdown.
#[must_use]
pub fn plan(
    old: &[Existing],
    new: Vec<NewBlock>,
    parent: Option<Id>,
    after: Option<Id>,
    lookup: &dyn Lookup,
    files_dir: Option<&str>,
) -> Merge {
    let mut planner = Planner {
        lookup,
        files_dir,
        merge: Merge::default(),
        inserts: Vec::new(),
        pairs_left: PAIRS_TOTAL_MAX,
    };
    planner.level(old, new, parent, after);
    let Planner {
        mut merge, inserts, ..
    } = planner;
    merge.ops.extend(inserts);
    merge
}

/// The blocks of a document as the tree they make, from the flat list a read gives: each with the blocks under it,
/// in order. Only live blocks.
#[must_use]
pub fn tree_of(blocks: &[DocumentBlock]) -> Vec<Existing> {
    let outline = blocks::Outline::new(blocks);
    fn build(outline: &blocks::Outline<'_>, block: &DocumentBlock) -> Existing {
        Existing {
            id: block.id,
            block_type: block.block_type,
            props: serde_json::to_value(&block.props)
                .unwrap_or_else(|_| Json::Object(serde_json::Map::new())),
            content: block.content.clone(),
            seq_updated: block.seq_updated,
            children: outline
                .children_of(block.id)
                .into_iter()
                .map(|child| build(outline, child))
                .collect(),
        }
    }
    outline
        .roots()
        .into_iter()
        .map(|block| build(&outline, block))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::NoLookup;
    use crate::markdown::{self, NoDirectory};
    use crate::operations::{self, Node, Tree};
    use crate::position::Position;

    /// The blocks a Markdown text makes, stored as a document holds them: positions, and a sequence of 1.
    fn stored(markdown: &str) -> Vec<DocumentBlock> {
        fn walk(into: &mut Vec<DocumentBlock>, parent: Option<Id>, siblings: &[NewBlock]) {
            for (index, new) in siblings.iter().enumerate() {
                let checked =
                    blocks::check(new.block_type, new.props.as_ref(), new.content.as_ref())
                        .unwrap();
                into.push(DocumentBlock {
                    id: new.id,
                    document_id: Id::nil(),
                    parent_id: parent,
                    position: (index + 1).to_string(),
                    block_type: new.block_type,
                    props: blocks::props_from_json(&checked.props),
                    content: checked.content,
                    removed: false,
                    seq_created: 1,
                    seq_updated: 1,
                    updated_by: None,
                    updated_at: Time::new(chrono::DateTime::UNIX_EPOCH),
                });
                walk(into, Some(new.id), new.children.as_deref().unwrap_or(&[]));
            }
        }
        let mut out = Vec::new();
        walk(&mut out, None, &markdown::read(markdown, &NoDirectory));
        out
    }

    fn merged(old_markdown: &str, new_markdown: &str) -> (Vec<DocumentBlock>, Merge) {
        let old = stored(old_markdown);
        let new = markdown::read(new_markdown, &NoDirectory);
        let merge = plan(&tree_of(&old), new, None, None, &NoLookup, None);
        (old, merge)
    }

    /// The ids of the blocks, in the order of the page, by the words each holds.
    fn ids_by_text(blocks: &[DocumentBlock]) -> HashMap<String, Id> {
        let outline = blocks::Outline::new(blocks);
        outline
            .walk()
            .into_iter()
            .map(|(block, _)| {
                (
                    blocks::plain_text(
                        block.block_type,
                        &serde_json::to_value(&block.props).unwrap(),
                        &block.content,
                    ),
                    block.id,
                )
            })
            .collect()
    }

    #[test]
    fn a_text_that_says_what_the_document_says_changes_nothing() {
        let text = "# Plan\n\nIntro.\n\n- one\n  - nested\n- two\n\n> [!NOTE]\n> careful\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n";
        let (_, merge) = merged(text, text);
        assert!(merge.is_empty(), "{:?}", merge.ops);
        assert_eq!(merge.kept, 7);
    }

    #[test]
    fn a_paragraph_that_was_edited_stays_the_same_block_and_its_neighbours_are_untouched() {
        let (old, merge) = merged(
            "# Plan\n\nFirst we build.\n\nThen we ship.\n\nLast, rest.\n",
            "# Plan\n\nFirst we build.\n\nThen we test and ship.\n\nLast, rest.\n",
        );
        let ids = ids_by_text(&old);
        assert_eq!(merge.ops.len(), 1, "{:?}", merge.ops);
        let DocumentOp::Update { id, content, .. } = &merge.ops[0] else {
            panic!("an update, not {:?}", merge.ops[0]);
        };
        assert_eq!(*id, ids["Then we ship."]);
        assert_eq!(
            blocks::inline_text(match content {
                Some(BlockContent::Inline(runs)) => runs,
                _ => panic!("text"),
            }),
            "Then we test and ship."
        );
    }

    #[test]
    fn what_markdown_cannot_say_is_kept_on_a_block_whose_words_changed_and_on_one_left_alone() {
        let mut old = stored("Coloured words.\n\nPlain words here.\n");
        for block in &mut old {
            if blocks::plain_text(
                block.block_type,
                &serde_json::to_value(&block.props).unwrap(),
                &block.content,
            ) == "Coloured words."
            {
                block.props = blocks::props_from_json(
                    &serde_json::json!({ "background": 3, "align": "center" }),
                );
            }
        }
        let unchanged = markdown::read("Coloured words.\n\nPlain words here.\n", &NoDirectory);
        let kept = plan(&tree_of(&old), unchanged, None, None, &NoLookup, None);
        assert!(kept.is_empty(), "{:?}", kept.ops);
        assert_eq!(kept.kept, 2);

        let explicit = markdown::read(
            "<!-- orch:block {\"kind\":\"paragraph\",\"props\":{\"background\":4,\"align\":\"right\"}} -->\n\nColoured words.\n\n<!-- /orch:block -->\n\nPlain words here.\n",
            &NoDirectory,
        );
        let styled = plan(&tree_of(&old), explicit, None, None, &NoLookup, None);
        assert_eq!(styled.updated, vec![old[0].id]);
        assert_eq!(styled.ops.len(), 1);
        let DocumentOp::Update { props, .. } = &styled.ops[0] else {
            panic!("explicit layout is an update");
        };
        let props = serde_json::to_value(props).unwrap();
        assert_eq!(props["background"], 4);
        assert_eq!(props["align"], "right");

        let new = markdown::read(
            "Coloured words, changed.\n\nPlain words here.\n",
            &NoDirectory,
        );
        let merge = plan(&tree_of(&old), new, None, None, &NoLookup, None);
        assert_eq!(merge.ops.len(), 1);
        let DocumentOp::Update { props, .. } = &merge.ops[0] else {
            panic!("an update");
        };
        let props = serde_json::to_value(props).unwrap();
        assert_eq!(props["background"], 3);
        assert_eq!(props["align"], "center");
    }

    fn tree_of_blocks(blocks: &[DocumentBlock]) -> Tree {
        Tree::new(
            blocks
                .iter()
                .map(|block| Node {
                    id: block.id,
                    parent: block.parent_id,
                    position: Position::parse(&block.position).unwrap(),
                    block_type: block.block_type,
                    seq_updated: block.seq_updated,
                    removed: false,
                    checked: Some(
                        blocks::check(block.block_type, Some(&block.props), Some(&block.content))
                            .unwrap(),
                    ),
                })
                .collect(),
        )
    }

    #[test]
    fn blocks_that_come_and_go_are_inserted_and_removed_in_place_and_the_order_is_the_texts() {
        let (old, merge) = merged(
            "alpha\n\nbeta\n\ngamma\n\ndelta\n",
            "alpha\n\ninserted before beta\n\nbeta\n\ndelta\n\nat the end\n",
        );
        let ids = ids_by_text(&old);
        assert_eq!(merge.removed, vec![ids["gamma"]]);
        assert_eq!(merge.inserted, 2);
        let mut tree = tree_of_blocks(&old);
        let effects = operations::apply(&mut tree, &merge.ops, 2, i64::MAX).unwrap();
        assert_eq!(effects.removed, vec![ids["gamma"]]);
        let mut words: HashMap<Id, String> =
            ids.iter().map(|(text, id)| (*id, text.clone())).collect();
        for written in &effects.inserted {
            words.insert(written.id, written.checked.text.clone());
        }
        let order: Vec<&str> = tree
            .kids(None)
            .iter()
            .map(|id| words[id].as_str())
            .collect();
        assert_eq!(
            order,
            [
                "alpha",
                "inserted before beta",
                "beta",
                "delta",
                "at the end"
            ]
        );
    }

    #[test]
    fn a_list_item_edited_keeps_its_children_and_a_block_of_another_kind_with_the_same_words_is_changed_in_place()
     {
        let (old, merge) = merged(
            "- buy milk\n  - oat\n- walk\n\nplain heading words\n",
            "- buy fresh milk\n  - oat\n- walk\n\n## plain heading words\n",
        );
        let ids = ids_by_text(&old);
        let updated: HashSet<Id> = merge.updated.iter().copied().collect();
        assert!(updated.contains(&ids["buy milk"]));
        assert!(updated.contains(&ids["plain heading words"]));
        assert!(merge.removed.is_empty(), "{:?}", merge.removed);
        assert_eq!(merge.inserted, 0);
        let kind = merge.ops.iter().find_map(|op| match op {
            DocumentOp::Update { id, block_type, .. } if *id == ids["plain heading words"] => {
                *block_type
            }
            _ => None,
        });
        assert_eq!(kind, Some(BlockType::Heading));
    }

    #[test]
    fn two_stretches_of_a_long_document_are_aligned_by_what_is_said_once() {
        let many = |tail: &str| -> String {
            let mut text = String::new();
            for number in 0..1_500 {
                text.push_str(&format!("paragraph number {number}\n\n"));
            }
            text.push_str(tail);
            text
        };
        let (old, merge) = merged(&many("end\n"), &many("a new end\n"));
        assert_eq!(old.len(), 1_501);
        assert_eq!(merge.ops.len(), 1, "one block changed: {}", merge.ops.len());
    }

    #[test]
    fn a_document_changed_in_every_block_is_laid_against_its_old_blocks_in_step_and_not_each_with_each()
     {
        let count = 20_000;
        let old: Vec<String> = (0..count)
            .map(|index| format!("Old paragraph {index} says one thing."))
            .collect();
        let new: Vec<String> = (0..count)
            .map(|index| format!("New paragraph {index} tells another."))
            .collect();
        let (blocks, merge) = merged(&old.join("\n\n"), &new.join("\n\n"));
        assert_eq!(blocks.len(), count);
        assert_eq!(
            merge.updated.len(),
            count,
            "every block is the block it was"
        );
        assert_eq!((merge.inserted, merge.removed.len(), merge.kept), (0, 0, 0));
    }
}
