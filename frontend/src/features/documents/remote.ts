/** Other people's changes laid over the page: rows the feed delivered become changes of the editor's state, block by block, without touching a block its person has an unsent change in, and without a trace in the person's own undo.
 *
 * Everything here is a function of a state and some rows, so it can be tested without a browser.
 * A block the person is changing keeps what they typed until their batch is answered; every other
 * block takes the row. Text that differs is replaced where it differs and nowhere else, so that a
 * caret in a line that somebody else is also writing in stays where it was.
 */
import type { JSONContent } from '@tiptap/core';
import type { Fragment, Node as PmNode } from '@tiptap/pm/model';
import type { EditorState, Transaction } from '@tiptap/pm/state';

import { sameBlock, type Tree } from '@/features/documents/blocks';
import { blockToJson, draftOf, newId, nodeOf, treeToJson } from '@/features/documents/schema';
import type { DocumentBlock } from '@/generated/api-types';

/** Marks a transaction as other people's work: it is not sent back, and it is not the person's to undo. */
export const REMOTE = 'orch-remote';

/** How many rows at once are taken as a new page instead of one change at a time. */
export const WHOLESALE_ROWS = 200;

/** Where a container stands in a document. */
export interface Found {
  pos: number;
  node: PmNode;
  parent: PmNode;
  /** The position before the parent, or -1 when the parent is the document. */
  parentPos: number;
  index: number;
}

/** The container with an identifier, or `null`. */
export function findContainer(doc: PmNode, id: string): Found | null {
  const walk = (parent: PmNode, parentPos: number, start: number): Found | null => {
    let pos = start;
    for (let index = 0; index < parent.childCount; index += 1) {
      const node = parent.child(index);
      if (node.attrs.id === id) return { pos, node, parent, parentPos, index };
      if (node.type.name === 'block' && node.childCount > 1) {
        const groupPos = pos + 1 + node.child(0).nodeSize;
        const inside = walk(node.child(1), groupPos, groupPos + 1);
        if (inside !== null) return inside;
      } else if (node.type.name === 'columns' || node.type.name === 'column') {
        const inside = walk(node, pos, pos + 1);
        if (inside !== null) return inside;
      }
      pos += node.nodeSize;
    }
    return null;
  };
  return walk(doc, -1, 0);
}

/** Every container of a document with where it stands, in the order of the page. */
export function containersOf(doc: PmNode): Map<string, Found> {
  const found = new Map<string, Found>();
  const walk = (parent: PmNode, parentPos: number, start: number) => {
    let pos = start;
    for (let index = 0; index < parent.childCount; index += 1) {
      const node = parent.child(index);
      found.set(String(node.attrs.id), { pos, node, parent, parentPos, index });
      if (node.type.name === 'block' && node.childCount > 1) {
        const groupPos = pos + 1 + node.child(0).nodeSize;
        walk(node.child(1), groupPos, groupPos + 1);
      } else if (node.type.name === 'columns' || node.type.name === 'column') walk(node, pos, pos + 1);
      pos += node.nodeSize;
    }
  };
  walk(doc, -1, 0);
  return found;
}

/** The identifier of the block a node stands under: the block that owns the group, the columns or the column, or `null` at the top. */
function parentIdOf(doc: PmNode, found: Found): string | null {
  if (found.parent.type.name === 'doc') return null;
  if (found.parent.type.name === 'blockGroup') {
    const owner = doc.resolve(found.parentPos).parent;
    return typeof owner.attrs.id === 'string' ? owner.attrs.id : null;
  }
  return typeof found.parent.attrs.id === 'string' ? found.parent.attrs.id : null;
}

export interface RemoteContext {
  /** The blocks as the server holds them now, these rows among them. */
  tree: Tree;
  /** The rows as they were held before these arrived. */
  before: ReadonlyMap<string, DocumentBlock>;
  /** Whether a container carries a change of its person's that the server has not answered. */
  isDirty: (id: string, node: PmNode) => boolean;
}

/** The editor's JSON for a block and everything under it, as the server holds them. */
export function subtreeJson(tree: Tree, id: string): JSONContent | null {
  const row = tree.rows.get(id);
  if (row === undefined) return null;
  const json = blockToJson({ id, type: row.type, props: row.props, content: row.content });
  const below = (tree.children.get(id) ?? []).flatMap((child) => {
    const one = subtreeJson(tree, child);
    return one === null ? [] : [one];
  });
  if (below.length === 0) return json;
  if (row.type === 'columns' || row.type === 'column') return { ...json, content: below };
  return { ...json, content: [...(json.content ?? []), { type: 'blockGroup', content: below }] };
}

function hasUnsent(node: PmNode, ctx: RemoteContext): boolean {
  let unsent = false;
  const visit = (current: PmNode) => {
    if (unsent) return;
    if (current.type.name === 'block' || current.type.name === 'leafBlock' || current.type.name === 'columns' || current.type.name === 'column') {
      const id = String(current.attrs.id);
      if (!ctx.tree.rows.has(id) || ctx.isDirty(id, current)) {
        unsent = true;
        return;
      }
    }
    current.forEach((child) => {
      if (!child.isText && !child.isInline) visit(child);
    });
  };
  visit(node);
  return unsent;
}

/** Takes a container away; a group it leaves empty goes with it. Its children stay, as siblings after its place, when `keep` says so. */
function takeAway(tr: Transaction, found: Found, keep: boolean): void {
  const node = found.node;
  const children: PmNode[] = [];
  if (keep && node.type.name === 'block' && node.childCount > 1) node.child(1).forEach((child) => children.push(child));
  const sole = found.parent.childCount === 1;
  if (sole && found.parent.type.name === 'blockGroup') {
    // The group is the last thing in its block: it goes with its only child.
    if (children.length > 0) tr.replaceWith(found.parentPos, found.parentPos + found.parent.nodeSize, children);
    else tr.delete(found.parentPos, found.parentPos + found.parent.nodeSize);
    return;
  }
  if (children.length > 0) tr.replaceWith(found.pos, found.pos + node.nodeSize, children);
  else if (sole && found.parent.type.name === 'doc') tr.replaceWith(found.pos, found.pos + node.nodeSize, nodeOf(tr.doc.type.schema, 'block').create({ id: newId() }, nodeOf(tr.doc.type.schema, 'paragraph').create()));
  else tr.delete(found.pos, found.pos + node.nodeSize);
}

/** Where a block goes among the children of its parent: after the nearest block before it in the server's order that the page has. */
function slotFor(tr: Transaction, ctx: RemoteContext, id: string, parentId: string | null): { pos: number; wrap: boolean } | null {
  const doc = tr.doc;
  const siblings = ctx.tree.children.get(parentId) ?? [];
  let holder: { start: number; node: PmNode; groupless: boolean; end: number };
  if (parentId === null) holder = { start: 0, node: doc, groupless: false, end: doc.content.size };
  else {
    const parent = findContainer(doc, parentId);
    if (parent === null) return null;
    if (parent.node.type.name === 'block') {
      if (parent.node.childCount > 1) {
        const groupPos = parent.pos + 1 + parent.node.child(0).nodeSize;
        holder = { start: groupPos + 1, node: parent.node.child(1), groupless: false, end: groupPos + parent.node.child(1).nodeSize - 1 };
      } else holder = { start: parent.pos + parent.node.nodeSize - 1, node: parent.node, groupless: true, end: parent.pos + parent.node.nodeSize - 1 };
    } else if (parent.node.type.name === 'leafBlock') return null;
    else holder = { start: parent.pos + 1, node: parent.node, groupless: false, end: parent.pos + parent.node.nodeSize - 1 };
  }
  if (holder.groupless) return { pos: holder.start, wrap: true };
  const at = siblings.indexOf(id);
  for (let before = at - 1; before >= 0; before -= 1) {
    const sibling = siblings[before];
    if (sibling === undefined) continue;
    const found = findContainer(doc, sibling);
    if (found !== null && (parentIdOf(doc, found) ?? null) === parentId) return { pos: found.pos + found.node.nodeSize, wrap: false };
  }
  return { pos: holder.start, wrap: false };
}

/** Replaces a text where it differs, so that a caret before or after the difference stays where it is. */
function replaceDifference(tr: Transaction, from: number, now: Fragment, next: Fragment): boolean {
  const start = now.findDiffStart(next);
  if (start === null) return false;
  const end = now.findDiffEnd(next);
  if (end === null) return false;
  let { a: endNow, b: endNext } = end;
  // The common start and the common end may overlap in a text that repeats: the difference is what is left.
  const overlap = start - Math.min(endNow, endNext);
  if (overlap > 0) {
    endNow += overlap;
    endNext += overlap;
  }
  const slice = next.cut(start, endNext);
  if (endNow > start || slice.size > 0) tr.replaceWith(from + start, from + endNow, slice);
  return true;
}

/** Makes the content node of a container say what a row says. */
export function updateContent(tr: Transaction, found: Found, row: DocumentBlock): void {
  const schema = tr.doc.type.schema;
  const container = found.node;
  if (row.type === 'columns') return;
  if (row.type === 'column') {
    const share = (row.props as { share?: number }).share ?? null;
    if (container.attrs.share !== share) tr.setNodeMarkup(found.pos, undefined, { ...container.attrs, share });
    return;
  }
  const json = blockToJson({ id: row.id, type: row.type, props: row.props, content: row.content });
  const next = schema.nodeFromJSON(json.content?.[0] ?? { type: 'paragraph' });
  const kindNow = container.type.name;
  if (json.type !== kindNow) {
    // A paragraph became code, or the other way: the block is made anew where it stood, and its children follow it where it can.
    const children = container.type.name === 'block' && container.childCount > 1 ? container.child(1).content.content : [];
    const made = schema.nodeFromJSON({ ...json, content: [next.toJSON() as JSONContent] });
    tr.replaceWith(found.pos, found.pos + container.nodeSize, [made, ...children]);
    return;
  }
  const content = container.child(0);
  const contentPos = found.pos + 1;
  if (content.type === next.type && JSON.stringify(content.attrs) === JSON.stringify(next.attrs) && next.type.name !== 'table') {
    replaceDifference(tr, contentPos + 1, content.content, next.content);
    return;
  }
  tr.replaceWith(contentPos, contentPos + content.nodeSize, next);
}

/**
 * Lays rows over the editor's state. A block with a change of its person's that is not answered yet
 * keeps it; every other block takes the row. Rows are applied removals first, then the blocks that
 * are there, then the blocks that are new, parents before what is under them.
 */
export function applyRows(state: EditorState, rows: readonly DocumentBlock[], ctx: RemoteContext): Transaction | null {
  const schema = state.schema;
  const tr = state.tr;
  tr.setMeta('addToHistory', false);
  tr.setMeta(REMOTE, true);

  for (const row of rows.filter((one) => one.removed)) {
    const found = findContainer(tr.doc, row.id);
    if (found === null) continue;
    // A block the person is writing in is theirs until their batch is answered, and answers with the block gone.
    if (ctx.isDirty(row.id, found.node)) continue;
    takeAway(tr, found, hasUnsent(found.node, ctx));
  }

  const depthOf = (id: string): number => {
    let depth = 0;
    for (let row = ctx.tree.rows.get(id); row !== undefined && row.parent_id !== null; row = ctx.tree.rows.get(row.parent_id)) depth += 1;
    return depth;
  };
  const live = rows.filter((one) => !one.removed && ctx.tree.rows.has(one.id)).sort((one, other) => depthOf(one.id) - depthOf(other.id));

  for (const row of live) {
    const found = findContainer(tr.doc, row.id);
    if (found === null) {
      const slot = slotFor(tr, ctx, row.id, row.parent_id);
      const json = subtreeJson(ctx.tree, row.id);
      if (slot === null || json === null) continue;
      const node = schema.nodeFromJSON(json);
      tr.insert(slot.pos, slot.wrap ? nodeOf(schema, 'blockGroup').create(null, node) : node);
      continue;
    }
    if (ctx.isDirty(row.id, found.node)) continue;
    const draft = draftOf(found.node, null);
    const held = ctx.before.get(row.id);
    if (!sameBlock(draft, row)) updateContent(tr, found, row);
    // The block was moved by somebody: it is taken out with everything under it and put where the server has it.
    if (held !== undefined && (held.parent_id !== row.parent_id || held.position !== row.position)) {
      const again = findContainer(tr.doc, row.id);
      if (again === null) continue;
      const node = again.node;
      takeAway(tr, again, false);
      const slot = slotFor(tr, ctx, row.id, row.parent_id);
      if (slot === null) continue;
      tr.insert(slot.pos, slot.wrap ? nodeOf(schema, 'blockGroup').create(null, node) : node);
    }
  }
  return tr.docChanged ? tr : null;
}

/** The whole page from the server's blocks, as one change. */
export function replaceAll(state: EditorState, tree: Tree): Transaction {
  const doc = state.schema.nodeFromJSON(treeToJson(tree));
  const tr = state.tr;
  tr.replaceWith(0, state.doc.content.size, doc.content);
  tr.setMeta('addToHistory', false);
  tr.setMeta(REMOTE, true);
  return tr;
}
