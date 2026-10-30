/** Which rows of a place are ticked, read from the few ticks that differ from their place, and the request that those ticks come to.
 *
 * Everything in a place is ticked as the dialog opens, and what the person changes is kept as the ticks that
 * differ: a row has the tick of the place it lies in unless it was ticked or cleared by itself. Ticking a place
 * takes what is in it, so it drops what was said about the rows below. A row is shown ticked when it and everything
 * below it is, half ticked when something below is, and empty otherwise. What the caller may read and not take
 * away is left out of every reading: the server leaves it out of the archive and counts it.
 *
 * The request is the largest places that are ticked whole, with the documents and the chats of the places that are
 * only partly ticked: a place that is ticked whole is asked for as itself, and "everything" as one word.
 */
import type { DocumentRef, TreeNode } from '@/generated/api-types';

/** The most identifiers one list of a request may name; a tree of ticks that comes to more is not asked for. */
export const CHOSEN_MAX = 10_000;

/** The nodes of the place the dialog is for, and what is written beside them, as far as it is known. */
export interface Scope {
  rootId: string;
  /** The nodes of the place, the place included, by identifier; none that was deleted. */
  nodes: ReadonlyMap<string, TreeNode>;
  /** The identifiers directly under each node, in the order they stand. */
  under: ReadonlyMap<string, readonly string[]>;
  /** The documents written beside a place, and whether the list holds all of them. */
  documents: (place: string) => { items: readonly DocumentRef[]; complete: boolean };
  /** What the caller reads and may not take away: nodes and documents, which no tick is laid on. */
  greyed: ReadonlySet<string>;
}

/** The order the kinds stand in under a place: the places inside it, its topics, then its threads. */
const KIND_ORDER: Record<TreeNode['kind'], number> = { org: 0, project: 1, topic: 2, thread: 3 };

/** Lays the nodes under `rootId` out as a scope: who stands under whom, in the order the left bar keeps. */
export function scopeOf(
  nodes: ReadonlyMap<string, TreeNode>,
  rootId: string,
  locale: string,
  documents: Scope['documents'],
  greyed: ReadonlySet<string>,
): Scope {
  const children = new Map<string, TreeNode[]>();
  for (const node of nodes.values()) {
    if (node.deleted || node.parent_id === null) continue;
    const list = children.get(node.parent_id);
    if (list === undefined) children.set(node.parent_id, [node]);
    else list.push(node);
  }
  const collator = new Intl.Collator(locale, { numeric: true, sensitivity: 'base' });
  const held = new Map<string, TreeNode>();
  const under = new Map<string, readonly string[]>();
  const visit = (node: TreeNode, seen: Set<string>) => {
    if (seen.has(node.id)) return;
    seen.add(node.id);
    held.set(node.id, node);
    const kids = (children.get(node.id) ?? []).toSorted((one, other) => KIND_ORDER[one.kind] - KIND_ORDER[other.kind] || collator.compare(one.name, other.name));
    under.set(
      node.id,
      kids.map((kid) => kid.id),
    );
    for (const kid of kids) visit(kid, seen);
  };
  const root = nodes.get(rootId);
  if (root !== undefined) visit(root, new Set());
  return { rootId, nodes: held, under, documents, greyed };
}

// ---------------------------------------------------------------- ticks

/** The ticks that differ from the place a row lies in, by identifier of a node or a document. */
export type Ticks = ReadonlyMap<string, boolean>;

/** What a row is: whether it is in, whether all it holds is, and whether any of it is. */
export interface Reading {
  on: boolean;
  all: boolean;
  any: boolean;
}

/** Reads every row of a scope against its ticks: the nodes, and the documents that are known. */
export function readTicks(scope: Scope, ticks: Ticks): Map<string, Reading> {
  const out = new Map<string, Reading>();
  const visit = (id: string, inherited: boolean): { all: boolean; any: boolean } => {
    const on = ticks.get(id) ?? inherited;
    if (scope.greyed.has(id)) {
      // A row nothing can be done with: it stands empty, and the rows round it are read without it.
      out.set(id, { on: false, all: false, any: false });
      return { all: true, any: false };
    }
    let all = on;
    let any = on;
    for (const document of scope.documents(id).items) {
      if (scope.greyed.has(document.id)) {
        out.set(document.id, { on: false, all: false, any: false });
        continue;
      }
      const inside = ticks.get(document.id) ?? on;
      out.set(document.id, { on: inside, all: inside, any: inside });
      all &&= inside;
      any ||= inside;
    }
    for (const child of scope.under.get(id) ?? []) {
      const part = visit(child, on);
      all &&= part.all;
      any ||= part.any;
    }
    out.set(id, { on, all, any });
    return { all, any };
  };
  if (scope.nodes.has(scope.rootId)) visit(scope.rootId, true);
  return out;
}

/** How a row is shown: ticked when all of it is in, half ticked when some of it is, empty otherwise. */
export function shownTick(reading: Reading | undefined): boolean | 'mixed' {
  if (reading === undefined) return false;
  return reading.all ? true : reading.any ? 'mixed' : false;
}

/** The identifiers of every row below a node: the nodes, and the documents that are known. */
function below(scope: Scope, id: string): string[] {
  const found: string[] = [];
  const visit = (at: string) => {
    for (const document of scope.documents(at).items) found.push(document.id);
    for (const child of scope.under.get(at) ?? []) {
      found.push(child);
      visit(child);
    }
  };
  visit(id);
  return found;
}

/** Drops the ticks that say what a row would have anyway: the same ticks come out whichever way they were reached. */
export function canonicalTicks(scope: Scope, ticks: Ticks): Ticks {
  const next = new Map(ticks);
  const visit = (id: string, inherited: boolean) => {
    const own = next.get(id);
    if (own === inherited) next.delete(id);
    const on = own ?? inherited;
    for (const document of scope.documents(id).items) if (next.get(document.id) === on) next.delete(document.id);
    for (const child of scope.under.get(id) ?? []) visit(child, on);
  };
  if (scope.nodes.has(scope.rootId)) visit(scope.rootId, true);
  // A tick on a row the scope no longer holds says nothing.
  const held = new Set<string>([...scope.nodes.keys()]);
  for (const id of scope.nodes.keys()) for (const document of scope.documents(id).items) held.add(document.id);
  for (const id of [...next.keys()]) if (!held.has(id)) next.delete(id);
  return next;
}

/** Ticks or clears a row, and with it everything below it: a place that is ticked takes what is in it. */
export function tickRow(scope: Scope, ticks: Ticks, id: string, on: boolean): Ticks {
  const next = new Map(ticks);
  next.set(id, on);
  for (const inside of below(scope, id)) next.delete(inside);
  return canonicalTicks(scope, next);
}

// ---------------------------------------------------------------- what the ticks come to

/** The things a request names one by one. */
export interface Chosen {
  nodes: string[];
  documents: string[];
  chats: string[];
}

/** What the ticks ask for: everything, nothing, or these. */
export type Wanted = { type: 'all' } | { type: 'nothing' } | { type: 'chosen'; chosen: Chosen };

export interface WantedReading {
  wanted: Wanted;
  /** Places whose documents must be known whole before the choice can be named: they are partly ticked and in themselves. */
  needDocuments: string[];
  /** A list of the request would pass what one list may name. */
  tooMany: boolean;
}

/**
 * What the ticks come to as a request, for the kinds of thing the person wants. `chats` and `documents` say whether those kinds are
 * wanted at all: a kind that is not wanted needs nothing named, and a place's documents are not asked for.
 */
export function wantedOf(scope: Scope, ticks: Ticks, kinds: { chats: boolean; documents: boolean }): WantedReading {
  const reading = readTicks(scope, ticks);
  const root = reading.get(scope.rootId);
  if (!root?.any) return { wanted: { type: 'nothing' }, needDocuments: [], tooMany: false };
  if (root.all) return { wanted: { type: 'all' }, needDocuments: [], tooMany: false };
  const chosen: Chosen = { nodes: [], documents: [], chats: [] };
  const need: string[] = [];
  const visit = (id: string) => {
    const here = reading.get(id);
    const node = scope.nodes.get(id);
    if (here === undefined || node === undefined || scope.greyed.has(id) || !here.any) return;
    if (here.all) {
      chosen.nodes.push(id);
      return;
    }
    // Only a part of it: its own things as they are ticked, and then its parts.
    if (here.on && kinds.chats && node.chat.seq > 0) chosen.chats.push(node.chat.id);
    if (kinds.documents) {
      const documents = scope.documents(id);
      if (here.on && !documents.complete) need.push(id);
      for (const document of documents.items) {
        if (reading.get(document.id)?.on === true && !scope.greyed.has(document.id)) chosen.documents.push(document.id);
      }
    }
    for (const child of scope.under.get(id) ?? []) visit(child);
  };
  visit(scope.rootId);
  const tooMany = [chosen.nodes, chosen.documents, chosen.chats].some((list) => list.length > CHOSEN_MAX);
  return { wanted: { type: 'chosen', chosen }, needDocuments: need, tooMany };
}

// ---------------------------------------------------------------- counting what is inside

/** What lies inside a node, for the words on its row. */
export interface Tally {
  /** The orgs and projects (folders) below it, at whatever depth. */
  places: number;
  topics: number;
  threads: number;
}

/** Counts what lies below every node of a scope, itself left out. */
export function tallies(scope: Scope): Map<string, Tally> {
  const out = new Map<string, Tally>();
  const visit = (id: string): Tally => {
    const total: Tally = { places: 0, topics: 0, threads: 0 };
    for (const child of scope.under.get(id) ?? []) {
      const kind = scope.nodes.get(child)?.kind;
      const inside = visit(child);
      total.places += inside.places + (kind === 'org' || kind === 'project' ? 1 : 0);
      total.topics += inside.topics + (kind === 'topic' ? 1 : 0);
      total.threads += inside.threads + (kind === 'thread' ? 1 : 0);
    }
    out.set(id, total);
    return total;
  };
  if (scope.nodes.has(scope.rootId)) visit(scope.rootId);
  return out;
}
