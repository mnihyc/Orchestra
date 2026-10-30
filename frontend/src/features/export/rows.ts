/** The rows of the tree in the dialog of a place: the place, what lies in it that is open, and the documents written beside each place that is open.
 *
 * A row is a place (an org, a project or a folder, a topic), a thread, or a document, with its box: ticked, empty
 * or half ticked, and locked when the caller may read the thing and not take it away. The rows are laid out flat,
 * as the tree control wants them, and a row whose thing did not change is handed back as the very same object, so
 * that a tick on one row draws one row.
 */
import type { DocumentRef, TreeNode } from '@/generated/api-types';
import type { TreeRow } from '@/ui/Tree';

import { type Reading, type Scope, shownTick, type Tally } from './selection';

/** What a row is of. */
export type RowItem =
  | {
      type: 'node';
      node: TreeNode;
      /** A project that lies in a project. */
      folder: boolean;
      tally: Tally;
      /** How many documents lie in it and in what lies in it, and whether that is the count of all of them. */
      documents: number;
      documentsComplete: boolean;
      /** How many things in this place the caller may read and cannot take away, as the plan says. */
      leftOut: number;
      greyed: boolean;
    }
  | { type: 'document'; document: DocumentRef; greyed: boolean };

/** What the last laying out gave each row, so that a row that did not change keeps the object it had. */
export type RowMemory = Map<string, RowItem>;

const same = (before: RowItem, after: RowItem): boolean => {
  if (before.type === 'node' && after.type === 'node') {
    return (
      before.node === after.node &&
      before.folder === after.folder &&
      before.tally.places === after.tally.places &&
      before.tally.topics === after.tally.topics &&
      before.tally.threads === after.tally.threads &&
      before.documents === after.documents &&
      before.documentsComplete === after.documentsComplete &&
      before.leftOut === after.leftOut &&
      before.greyed === after.greyed
    );
  }
  return before.type === 'document' && after.type === 'document' && before.document === after.document && before.greyed === after.greyed;
};

function kept(memory: RowMemory, id: string, made: RowItem): RowItem {
  const held = memory.get(id);
  if (held !== undefined && same(held, made)) return held;
  memory.set(id, made);
  return made;
}

export interface LayoutInput {
  scope: Scope;
  expanded: ReadonlySet<string>;
  reading: ReadonlyMap<string, Reading>;
  tallies: ReadonlyMap<string, Tally>;
  /** The documents written beside each place, and in all below it. */
  documentsBelow: ReadonlyMap<string, number>;
  documentsComplete: boolean;
  /** The things the plan says were left out, by the place they lie in. */
  leftOutIn: ReadonlyMap<string, number>;
  memory: RowMemory;
}

const NO_TALLY: Tally = { places: 0, topics: 0, threads: 0 };

/** Lays the tree out as the rows in view, from the place down. */
export function layoutRows(input: LayoutInput): TreeRow<RowItem>[] {
  const { scope, expanded, reading, memory } = input;
  const rows: TreeRow<RowItem>[] = [];
  const visit = (id: string, depth: number, parent: string | null) => {
    const node = scope.nodes.get(id);
    if (node === undefined) return;
    const parentNode = node.parent_id === null ? undefined : scope.nodes.get(node.parent_id);
    const kids = scope.under.get(id) ?? [];
    const documents = node.kind === 'thread' ? [] : scope.documents(id).items;
    const open = expanded.has(id);
    const greyed = scope.greyed.has(id);
    const item = kept(memory, id, {
      type: 'node',
      node,
      folder: node.kind === 'project' && parentNode?.kind === 'project',
      tally: input.tallies.get(id) ?? NO_TALLY,
      documents: input.documentsBelow.get(id) ?? 0,
      documentsComplete: input.documentsComplete,
      leftOut: input.leftOutIn.get(id) ?? 0,
      greyed,
    });
    rows.push({
      id,
      depth,
      item,
      // Anything but a thread may hold documents, which are not known to be none until they have all been read.
      expandable: node.kind !== 'thread' && (kids.length > 0 || documents.length > 0 || !input.documentsComplete),
      expanded: open,
      parentId: parent,
      tick: greyed ? false : shownTick(reading.get(id)),
      tickLocked: greyed,
    });
    if (!open) return;
    for (const kid of kids) visit(kid, depth + 1, id);
    for (const document of documents) {
      const locked = scope.greyed.has(document.id);
      rows.push({
        id: document.id,
        depth: depth + 1,
        item: kept(memory, document.id, { type: 'document', document, greyed: locked }),
        expandable: false,
        expanded: false,
        parentId: id,
        tick: locked ? false : shownTick(reading.get(document.id)),
        tickLocked: locked,
      });
    }
  };
  visit(scope.rootId, 0, null);
  return rows;
}

/** How many documents lie beside each node and in everything below it, from the lists as they stand. */
export function documentsBelow(scope: Scope, byPlace: ReadonlyMap<string, readonly unknown[]>): Map<string, number> {
  const out = new Map<string, number>();
  const visit = (id: string): number => {
    let count = byPlace.get(id)?.length ?? 0;
    for (const child of scope.under.get(id) ?? []) count += visit(child);
    out.set(id, count);
    return count;
  };
  if (scope.nodes.has(scope.rootId)) visit(scope.rootId);
  return out;
}
