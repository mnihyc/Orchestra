/** What the keys and the menus do to a page of blocks: end a block and start the next, join one to the one before, put a block under the one above and take it out again, turn it into another type, remove it.
 *
 * The editor's tree is the document's tree: a block is a container with a content node and, when it
 * has any, a group of children. ProseMirror's own commands know nothing of that shape, so the habits
 * of a block editor are written here, each as a command that takes the state and, when it can act,
 * says so and dispatches one transaction.
 */
import { type Fragment, type Node as PmNode, type NodeType } from '@tiptap/pm/model';
import { NodeSelection, Selection, TextSelection, type EditorState, type Transaction } from '@tiptap/pm/state';

import { NESTING_MAX } from '@/features/documents/blocks';
import { KINDS, newId, nodeOf, TEXT_NODES } from '@/features/documents/schema';

export type Command = (state: EditorState, dispatch?: (tr: Transaction) => void) => boolean;

/** The identifier a container has, or a new one for a container that has none yet. */
const idOf = (node: PmNode): string => (typeof node.attrs.id === 'string' ? node.attrs.id : newId());

const isContainer = (node: PmNode): boolean => node.type.name === 'block' || node.type.name === 'leafBlock';
export const isTextContent = (node: PmNode): boolean => TEXT_NODES.includes(node.type.name);

/** Where a container stands: before it, in its parent, and at which index. */
export interface Where {
  depth: number;
  pos: number;
  node: PmNode;
  parent: PmNode;
  index: number;
}

/** The container a position is in. */
export function containerAt(doc: PmNode, pos: number): Where | null {
  const $pos = doc.resolve(pos);
  for (let depth = $pos.depth; depth > 0; depth -= 1) {
    const node = $pos.node(depth);
    if (isContainer(node)) return { depth, pos: $pos.before(depth), node, parent: $pos.node(depth - 1), index: $pos.index(depth - 1) };
  }
  return null;
}

/** The container that starts at a position. */
export function containerStartingAt(doc: PmNode, pos: number): Where | null {
  const $pos = doc.resolve(pos);
  const node = $pos.nodeAfter;
  if (node === null || (!isContainer(node) && node.type.name !== 'columns' && node.type.name !== 'column')) return null;
  return { depth: $pos.depth + 1, pos, node, parent: $pos.parent, index: $pos.index() };
}

/** The attributes of a text block that stay when it becomes another type: its look. */
function looksOf(content: PmNode): Record<string, unknown> {
  return { color: content.attrs.color ?? null, background: content.attrs.background ?? null, align: content.attrs.align ?? null };
}

/** How many blocks a position is under. */
function ancestorsOf(doc: PmNode, pos: number): number {
  const $pos = doc.resolve(pos);
  let count = 0;
  for (let depth = $pos.depth; depth > 0; depth -= 1) {
    const name = $pos.node(depth).type.name;
    if (name === 'block' || name === 'columns' || name === 'column') count += 1;
  }
  return count;
}

/** How many levels of blocks a container holds, itself among them. */
function heightOf(node: PmNode): number {
  let most = 0;
  const walk = (current: PmNode, level: number) => {
    most = Math.max(most, level);
    current.forEach((child) => {
      if (child.type.name === 'blockGroup') child.forEach((item) => walk(item, level + 1));
      else if (child.type.name === 'column') walk(child, level + 1);
    });
  };
  walk(node, 1);
  return most;
}

// ---------------------------------------------------------------- Enter

/**
 * Ends a block and starts the next of its kind. On an empty block that is no paragraph it turns the
 * block back into a paragraph. At the end of a block that has children the new block is the first
 * of them; in the middle of one, the words after the caret go to the new block and take the children.
 */
export const splitBlock: Command = (state, dispatch) => {
  const tr = state.tr;
  if (!state.selection.empty) tr.deleteSelection();
  const $from = tr.doc.resolve(tr.selection.from);
  const at = containerAt(tr.doc, $from.pos);
  const content = $from.parent;
  if (at?.node.type.name !== 'block' || !isTextContent(content)) return false;
  const { schema } = state;
  const offset = $from.parentOffset;
  const empty = content.content.size === 0;
  const atEnd = offset === content.content.size;

  if (empty && content.type.name !== 'paragraph') {
    // The block was a list item, a heading, a quote: Enter on nothing leaves it.
    tr.setNodeMarkup(at.pos + 1, schema.nodes.paragraph, looksOf(content));
    dispatch?.(tr.scrollIntoView());
    return true;
  }

  const name = content.type.name;
  const nextType: NodeType = atEnd && (name === 'heading' || name === 'callout') ? nodeOf(schema, 'paragraph') : content.type;
  const nextAttrs: Record<string, unknown> =
    nextType.name === 'paragraph' && name !== 'paragraph'
      ? {}
      : { ...content.attrs, checked: name === 'todo' ? false : undefined, start: name === 'numbered' ? null : undefined };
  const filled = (type: NodeType, attrs: Record<string, unknown>, children: Fragment) => type.create(Object.fromEntries(Object.entries(attrs).filter(([, value]) => value !== undefined)), children);

  const tail = content.content.cut(offset);
  const head = content.content.cut(0, offset);
  const group = at.node.childCount > 1 ? at.node.child(1) : null;
  const blockType = nodeOf(schema, 'block');

  if (atEnd && group !== null) {
    // The new block is the first of the children.
    const created = blockType.create({ id: newId() }, filled(nextType, nextAttrs, tail));
    const first = at.pos + 1 + content.nodeSize + 1;
    tr.insert(first, created);
    tr.setSelection(TextSelection.create(tr.doc, first + 2));
  } else {
    const kept = at.node.type.create(at.node.attrs, [content.type.create(content.attrs, head, content.marks), ...(atEnd && group !== null ? [group] : [])]);
    const created = blockType.create({ id: newId() }, [filled(nextType, nextAttrs, tail), ...(!atEnd && group !== null ? [group] : [])]);
    tr.replaceWith(at.pos, at.pos + at.node.nodeSize, [kept, created]);
    tr.setSelection(TextSelection.create(tr.doc, at.pos + kept.nodeSize + 2));
  }
  dispatch?.(tr.scrollIntoView());
  return true;
};

// ---------------------------------------------------------------- Backspace and Delete

/** The last block that is shown under a container: itself, or the last of its children, deeply. */
function lastShown(node: PmNode, pos: number): { node: PmNode; pos: number } {
  let current = node;
  let at = pos;
  while (current.type.name === 'block' && current.childCount > 1) {
    const group = current.child(1);
    const last = group.lastChild;
    if (last === null) break;
    at = at + 1 + current.child(0).nodeSize + 1 + (group.content.size - last.nodeSize);
    current = last;
  }
  return { node: current, pos: at };
}

/** Takes a block out from under its parent: it comes after the parent, and the blocks that followed it become its children. */
export const liftBlock: Command = (state, dispatch) => {
  const at = containerAt(state.doc, state.selection.from) ?? containerStartingAt(state.doc, state.selection.from);
  if (at?.parent.type.name !== 'blockGroup') return false;
  // The block the group belongs to is one level above the group, wherever in the group this block stands.
  const $at = state.doc.resolve(at.pos);
  const parentNode = $at.depth > 0 ? $at.node($at.depth - 1) : null;
  if (parentNode?.type.name !== 'block') return false;
  const parentAt = { node: parentNode, pos: $at.before($at.depth - 1) };
  const group = at.parent;
  const before: PmNode[] = [];
  const after: PmNode[] = [];
  group.forEach((child, _offset, index) => {
    if (index < at.index) before.push(child);
    else if (index > at.index) after.push(child);
  });
  const { schema } = state;
  const keptParent = parentAt.node.type.create(parentAt.node.attrs, [parentAt.node.child(0), ...(before.length > 0 ? [nodeOf(schema, 'blockGroup').create(null, before)] : [])]);
  let moved = at.node;
  const leaf = at.node.type.name !== 'block';
  const sisters: PmNode[] = [];
  if (after.length > 0) {
    if (leaf) sisters.push(...after);
    else {
      const own = at.node.childCount > 1 ? at.node.child(1) : null;
      const children = [...(own === null ? [] : own.content.content), ...after];
      moved = at.node.type.create(at.node.attrs, [at.node.child(0), nodeOf(schema, 'blockGroup').create(null, children)]);
    }
  }
  const tr = state.tr;
  const offsetInBlock = state.selection.from - at.pos;
  tr.replaceWith(parentAt.pos, parentAt.pos + parentAt.node.nodeSize, [keptParent, moved, ...sisters]);
  tr.setSelection(Selection.near(tr.doc.resolve(Math.min(tr.doc.content.size, parentAt.pos + keptParent.nodeSize + offsetInBlock)), 1));
  dispatch?.(tr.scrollIntoView());
  return true;
};

/** Puts a block under the one above it, as its last child. */
export const sinkBlock: Command = (state, dispatch) => {
  const at = containerAt(state.doc, state.selection.from) ?? containerStartingAt(state.doc, state.selection.from);
  if (at === null || at.index === 0 || (at.node.type.name !== 'block' && at.node.type.name !== 'leafBlock')) return false;
  const previous = at.parent.child(at.index - 1);
  if (previous.type.name !== 'block') return false;
  const previousPos = at.pos - previous.nodeSize;
  if (ancestorsOf(state.doc, previousPos) + 1 + heightOf(at.node) > NESTING_MAX + 1) return false;
  const { schema } = state;
  const group = previous.childCount > 1 ? previous.child(1) : null;
  const grown = previous.type.create(previous.attrs, [
    previous.child(0),
    nodeOf(schema, 'blockGroup').create(null, group === null ? [at.node] : [...group.content.content, at.node]),
  ]);
  const tr = state.tr;
  const offsetInBlock = state.selection.from - at.pos;
  tr.replaceWith(previousPos, at.pos + at.node.nodeSize, grown);
  const newPos = previousPos + grown.nodeSize - 2 - at.node.nodeSize;
  tr.setSelection(Selection.near(tr.doc.resolve(newPos + offsetInBlock), 1));
  dispatch?.(tr.scrollIntoView());
  return true;
};

/** Joins the words of a block to the block shown before it, which takes the children too. Answers whether it did. */
function mergeIntoPrevious(state: EditorState, at: Where, dispatch?: (tr: Transaction) => void): boolean {
  const previous = at.parent.child(at.index - 1);
  const previousPos = at.pos - previous.nodeSize;
  const shown = lastShown(previous, previousPos);
  const content = at.node.child(0);
  const ourGroup = at.node.childCount > 1 ? at.node.child(1) : null;
  if (shown.node.type.name === 'block') {
    const tr = state.tr;
    tr.delete(at.pos, at.pos + at.node.nodeSize);
    const own = shown.node.child(0);
    const merged = shown.node.type.create(shown.node.attrs, [own.copy(own.content.append(content.content)), ...(ourGroup === null ? [] : [ourGroup])]);
    tr.replaceWith(shown.pos, shown.pos + shown.node.nodeSize, merged);
    tr.setSelection(TextSelection.create(tr.doc, shown.pos + 2 + own.content.size));
    dispatch?.(tr.scrollIntoView());
    return true;
  }
  const tr = state.tr;
  if (content.content.size === 0) {
    // An empty paragraph after a block that is no text: it goes, and the caret goes to the end of that block.
    tr.delete(at.pos, at.pos + at.node.nodeSize);
    const target = Selection.findFrom(tr.doc.resolve(Math.min(tr.doc.content.size, at.pos)), -1);
    if (target !== null) tr.setSelection(target);
    dispatch?.(tr.scrollIntoView());
    return true;
  }
  // Words after an image, a table, a diagram: the first press selects it, the next takes it away.
  if (shown.node.type.name === 'leafBlock' || shown.node.type.name === 'columns') {
    tr.setSelection(NodeSelection.create(tr.doc, shown.pos));
    dispatch?.(tr.scrollIntoView());
    return true;
  }
  return false;
}

/**
 * Backspace at the start of a block turns it back into text; at the start of text it takes the block
 * out from under its parent when it is the first child, and otherwise joins it to the one above.
 */
export const backspaceAtStart: Command = (state, dispatch) => {
  const { $from, empty } = state.selection;
  if (!empty || $from.parentOffset !== 0) return false;
  const at = containerAt(state.doc, $from.pos);
  if (at === null) return false;
  const content = $from.parent;
  const { schema } = state;

  if (at.node.type.name === 'leafBlock') {
    // Code and captions: an empty code block becomes text again; nothing else is joined to what is above.
    if (content.type.name === 'codeBlock' && content.content.size === 0) {
      const tr = state.tr;
      tr.replaceWith(at.pos, at.pos + at.node.nodeSize, nodeOf(schema, 'block').create({ id: idOf(at.node) }, nodeOf(schema, 'paragraph').create()));
      tr.setSelection(TextSelection.create(tr.doc, at.pos + 2));
      dispatch?.(tr);
    }
    return true;
  }
  if (!isTextContent(content)) return false;

  if (content.type.name !== 'paragraph') {
    const tr = state.tr;
    tr.setNodeMarkup(at.pos + 1, schema.nodes.paragraph, looksOf(content));
    dispatch?.(tr);
    return true;
  }
  if (at.index > 0) return mergeIntoPrevious(state, at, dispatch);
  if (at.parent.type.name === 'blockGroup') return liftBlock(state, dispatch);
  // The first block of a page or of a column has nothing above it.
  return true;
};

/** Delete at the end of a block joins the next block to it, when that is a block of text. */
export const deleteAtEnd: Command = (state, dispatch) => {
  const { $from, empty } = state.selection;
  if (!empty || $from.parentOffset !== $from.parent.content.size) return false;
  const at = containerAt(state.doc, $from.pos);
  if (at?.node.type.name !== 'block' || !isTextContent($from.parent)) return false;
  // Children come first on the page: their first is what follows this block's text.
  if (at.node.childCount > 1) return true;
  if (at.index + 1 >= at.parent.childCount) return true;
  const next = at.parent.child(at.index + 1);
  if (next.type.name !== 'block') return true;
  const nextAt: Where = { depth: at.depth, pos: at.pos + at.node.nodeSize, node: next, parent: at.parent, index: at.index + 1 };
  return mergeIntoPrevious(state, nextAt, dispatch);
};

// ---------------------------------------------------------------- turning, inserting, removing

/** A text block's look, carried to another type. */
export function carriedAttrs(from: PmNode, to: NodeType): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const key of Object.keys(to.spec.attrs ?? {})) if (key in from.attrs && ['color', 'background', 'align'].includes(key)) out[key] = from.attrs[key];
  return out;
}

/** Turns the block at `pos` into another type that holds text, keeping its words and its children. Works on the transaction it is given. */
export function turnInto(tr: Transaction, pos: number, nodeName: string, attrs: Record<string, unknown> = {}): boolean {
  const at = containerStartingAt(tr.doc, pos);
  if (at === null || (at.node.type.name !== 'block' && at.node.type.name !== 'leafBlock')) return false;
  const schema = tr.doc.type.schema;
  const type = schema.nodes[nodeName];
  const content = at.node.child(0);
  if (type === undefined) return false;
  if (nodeName === 'codeBlock') {
    const text = content.textContent;
    const code = type.create({ language: '', ...attrs }, text === '' ? undefined : schema.text(text));
    const group = at.node.childCount > 1 ? at.node.child(1) : null;
    tr.replaceWith(at.pos, at.pos + at.node.nodeSize, [nodeOf(schema, 'leafBlock').create({ id: idOf(at.node) }, code), ...(group === null ? [] : group.content.content)]);
    return true;
  }
  if (content.type.name === 'codeBlock') {
    const text = content.textContent;
    const next = type.create(attrs, text === '' ? undefined : schema.text(text));
    tr.replaceWith(at.pos, at.pos + at.node.nodeSize, nodeOf(schema, 'block').create({ id: idOf(at.node) }, next));
    return true;
  }
  if (!isTextContent(content)) return false;
  tr.setNodeMarkup(at.pos + 1, type, { ...carriedAttrs(content, type), ...attrs });
  return true;
}

/** Puts a block after the one at `pos`, and the caret in it when it holds text. */
export function insertAfter(state: EditorState, pos: number, block: PmNode): Transaction | null {
  const at = containerStartingAt(state.doc, pos);
  if (at === null) return null;
  const tr = state.tr;
  const where = at.pos + at.node.nodeSize;
  tr.insert(where, block);
  const first = block.type.name === 'block' || block.type.name === 'leafBlock' ? block.child(0) : null;
  tr.setSelection(first !== null && (isTextContent(first) || first.type.name === 'codeBlock') ? TextSelection.create(tr.doc, where + 2) : NodeSelection.create(tr.doc, where));
  return tr;
}

/** Removes the block at `pos` with everything under it, and a group that it leaves empty. */
export function removeBlock(state: EditorState, pos: number): Transaction | null {
  const at = containerStartingAt(state.doc, pos);
  if (at === null) return null;
  const tr = state.tr;
  const sole = at.parent.childCount === 1;
  if (sole && at.parent.type.name === 'blockGroup') {
    // The group goes with its last child, and the parent block is left as it was without children.
    tr.delete(at.pos - 1, at.pos + at.node.nodeSize + 1);
  } else if (sole && at.parent.type.name === 'doc') {
    tr.replaceWith(at.pos, at.pos + at.node.nodeSize, nodeOf(state.schema, 'block').create({ id: newId() }, nodeOf(state.schema, 'paragraph').create()));
  } else {
    tr.delete(at.pos, at.pos + at.node.nodeSize);
  }
  const near = Selection.findFrom(tr.doc.resolve(Math.min(Math.max(0, at.pos), tr.doc.content.size)), -1) ?? Selection.findFrom(tr.doc.resolve(Math.min(Math.max(0, at.pos), tr.doc.content.size)), 1);
  if (near !== null) tr.setSelection(near);
  return tr;
}

/** A copy of a block with everything under it, each with an identifier of its own. */
export function copyOf(node: PmNode): PmNode {
  const children: PmNode[] = [];
  node.forEach((child) => children.push(child.type.name === 'blockGroup' || child.type.name === 'column' ? copyOf(child) : child));
  const attrs = 'id' in node.attrs ? { ...node.attrs, id: newId() } : node.attrs;
  return node.type.create(attrs, children, node.marks);
}

/** Puts a copy of the block at `pos` after it. */
export function duplicateBlock(state: EditorState, pos: number): Transaction | null {
  const at = containerStartingAt(state.doc, pos);
  if (at === null) return null;
  const tr = state.tr;
  tr.insert(at.pos + at.node.nodeSize, copyOf(at.node));
  return tr;
}

/** The block selected as a whole (a click on a divider or an image) goes, and Backspace or Delete does that. */
export const deleteSelectedBlock: Command = (state, dispatch) => {
  const { selection } = state;
  if (!(selection instanceof NodeSelection) || (!isContainer(selection.node) && selection.node.type.name !== 'columns')) return false;
  const tr = removeBlock(state, selection.from);
  if (tr === null) return false;
  dispatch?.(tr.scrollIntoView());
  return true;
};

/** Mod+Enter ticks a to-do and leaves a block of code for the paragraph after it. */
export const quickAction: Command = (state, dispatch) => {
  const { $from } = state.selection;
  const content = $from.parent;
  const at = containerAt(state.doc, $from.pos);
  if (at === null) return false;
  if (content.type.name === 'todo') {
    dispatch?.(state.tr.setNodeAttribute(at.pos + 1, 'checked', content.attrs.checked !== true));
    return true;
  }
  if (content.type.name === 'codeBlock') {
    const paragraph = nodeOf(state.schema, 'block').create({ id: newId() }, nodeOf(state.schema, 'paragraph').create());
    const tr = insertAfter(state, at.pos, paragraph);
    if (tr === null) return false;
    dispatch?.(tr.scrollIntoView());
    return true;
  }
  return false;
};

/** The type of the content node of a block of text or a leaf, by the block type's name. */
export function nodeNameOf(type: keyof typeof KINDS): string {
  return KINDS[type].node;
}
