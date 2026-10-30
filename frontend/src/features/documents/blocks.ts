/** Orch's blocks as plain data: their order, the tree they make, their canonical form, and the batch of operations that turns the blocks the server holds into the blocks a person has made.
 *
 * Nothing here knows an editor. The editor's state is turned into drafts at the edge of one component
 * (`schema.ts`), and everything that decides what is sent, what differs and where a block stands
 * works on these plain values, so that it can be tested without a browser and the editor replaced
 * without touching what a document is.
 */
import type { BlockType, DocumentBlock, DocumentOp, Inline, TableRow } from '../../generated/api-types.ts';

export type { BlockType };
export type Props = Record<string, unknown>;
export type Content = Inline[] | TableRow[];

/** A block as a person has it on the page: what is compared with the server's block and what is sent. */
export interface Draft {
  id: string;
  parentId: string | null;
  type: BlockType;
  props: Props;
  content: Content;
}

/** The most operations the server takes in one batch. */
export const OPS_PER_BATCH = 500;
/** The most ancestors a block may have: sixteen indents are as many as a reading column can show. */
export const NESTING_MAX = 16;
/** The block types that may not have children. */
export const LEAF_TYPES: ReadonlySet<BlockType> = new Set<BlockType>([
  'code',
  'math',
  'diagram',
  'divider',
  'page_break',
  'image',
  'video',
  'audio',
  'file',
  'table',
]);
/** The block types whose content is text with marks (the others have a source, a caption of their own, rows, or nothing). */
export const TEXT_TYPES: ReadonlySet<BlockType> = new Set<BlockType>([
  'paragraph',
  'heading',
  'bullet',
  'numbered',
  'todo',
  'toggle',
  'quote',
  'callout',
]);

// ---------------------------------------------------------------- order

/**
 * Orders two positions: signed decimal numbers written as text, of any length. They are compared as
 * numbers without being converted to one, since a position that has been halved many times has more
 * digits than a double holds.
 */
export function comparePositions(one: string, other: string): number {
  const a = parsePosition(one);
  const b = parsePosition(other);
  if (a.negative !== b.negative) return a.negative ? -1 : 1;
  const magnitude = compareMagnitudes(a, b);
  return a.negative ? -magnitude : magnitude;
}

interface Position {
  negative: boolean;
  whole: string;
  fraction: string;
}

function parsePosition(text: string): Position {
  const found = /^(-)?(\d*)(?:\.(\d*))?$/.exec(text.trim());
  if (found === null) return { negative: false, whole: '0', fraction: '' };
  const whole = (found[2] ?? '').replace(/^0+/, '');
  const fraction = (found[3] ?? '').replace(/0+$/, '');
  // Minus zero is zero.
  return { negative: found[1] === '-' && (whole !== '' || fraction !== ''), whole, fraction };
}

function compareMagnitudes(a: Position, b: Position): number {
  if (a.whole.length !== b.whole.length) return a.whole.length < b.whole.length ? -1 : 1;
  if (a.whole !== b.whole) return a.whole < b.whole ? -1 : 1;
  if (a.fraction === b.fraction) return 0;
  return a.fraction < b.fraction ? -1 : 1;
}

/** Orders blocks among their siblings: by position, and equal positions by identifier. */
export function compareBlocks(one: Pick<DocumentBlock, 'position' | 'id'>, other: Pick<DocumentBlock, 'position' | 'id'>): number {
  const byPosition = comparePositions(one.position, other.position);
  if (byPosition !== 0) return byPosition;
  return one.id < other.id ? -1 : one.id > other.id ? 1 : 0;
}

// ---------------------------------------------------------------- the tree

/** The live blocks of a document, with each parent's children in order. */
export interface Tree {
  rows: ReadonlyMap<string, DocumentBlock>;
  /** The children of a parent (`null`: the top level), as identifiers in order. */
  children: ReadonlyMap<string | null, readonly string[]>;
}

/** Builds the tree of the live blocks among `rows`. A block whose parent is not live hangs from nothing and is left out. */
export function buildTree(rows: Iterable<DocumentBlock>): Tree {
  const live = new Map<string, DocumentBlock>();
  for (const row of rows) if (!row.removed) live.set(row.id, row);
  const grouped = new Map<string | null, DocumentBlock[]>();
  for (const row of live.values()) {
    if (row.parent_id !== null && !live.has(row.parent_id)) continue;
    const siblings = grouped.get(row.parent_id);
    if (siblings === undefined) grouped.set(row.parent_id, [row]);
    else siblings.push(row);
  }
  const children = new Map<string | null, string[]>();
  for (const [parent, siblings] of grouped) children.set(parent, siblings.sort(compareBlocks).map((row) => row.id));
  return { rows: live, children };
}

/** The identifiers of a tree in the order of the page: each block before what is under it. */
export function preOrder(tree: Tree): string[] {
  const found: string[] = [];
  const walk = (parent: string | null) => {
    for (const id of tree.children.get(parent) ?? []) {
      found.push(id);
      walk(id);
    }
  };
  walk(null);
  return found;
}

/** The blocks of a tree as drafts, in the order of the page. */
export function draftsOf(tree: Tree): Draft[] {
  return preOrder(tree).flatMap((id) => {
    const row = tree.rows.get(id);
    return row === undefined ? [] : [{ id, parentId: row.parent_id, type: row.type, props: row.props, content: row.content }];
  });
}

/** How many ancestors a block has in a list of drafts. */
export function depthsOf(drafts: readonly Draft[]): Map<string, number> {
  const depths = new Map<string, number>();
  for (const draft of drafts) depths.set(draft.id, draft.parentId === null ? 0 : (depths.get(draft.parentId) ?? 0) + 1);
  return depths;
}

// ---------------------------------------------------------------- canonical form

/** What a prop has when it is left out: it is not stored, so that "left" and nothing are the same block. */
const PROP_DEFAULTS: Readonly<Record<string, unknown>> = { align: 'left', wrap: false, toggle: false, header_column: false };

/** The shares of a table's columns as percentages with a decimal: dragging a column's edge and drawing it in pixels both come to the same shares. */
function proportions(widths: readonly number[]): number[] {
  const total = widths.reduce((sum, width) => sum + width, 0);
  if (total <= 0) return widths.slice();
  return widths.map((width) => Math.round((width / total) * 1000) / 10);
}

const MARK_ORDER = ['bold', 'italic', 'underline', 'strike', 'code'] as const;

function sortedEntries(value: Record<string, unknown>): [string, unknown][] {
  return Object.entries(value).sort(([one], [other]) => (one < other ? -1 : one > other ? 1 : 0));
}

/** A block's props in the form that is compared and sent: nothing set to nothing or to its default, keys in order. */
export function canonProps(props: unknown): Props {
  const out: Props = {};
  if (typeof props !== 'object' || props === null) return out;
  for (const [key, value] of sortedEntries(props as Record<string, unknown>)) {
    if (value === null || value === undefined) continue;
    if (key in PROP_DEFAULTS && PROP_DEFAULTS[key] === value) continue;
    if (key === 'widths' && Array.isArray(value)) out[key] = proportions(value as number[]);
    else if (Array.isArray(value)) out[key] = value.slice();
    else out[key] = value;
  }
  return out;
}

function canonInline(run: Inline): Inline {
  switch (run.type) {
    case 'text': {
      const out: Record<string, unknown> = { type: 'text', text: run.text };
      const marks = MARK_ORDER.filter((mark) => run.marks?.includes(mark));
      if (marks.length > 0) out.marks = marks;
      if (typeof run.href === 'string' && run.href !== '') out.href = run.href;
      if (typeof run.color === 'number') out.color = run.color;
      if (typeof run.background === 'number') out.background = run.background;
      return out as Inline;
    }
    case 'mention':
      return { type: 'mention', user_id: run.user_id };
    case 'ref':
      return { type: 'ref', kind: run.kind, id: run.id, ...(run.part === undefined ? {} : { part: run.part }) };
    case 'math':
      return { type: 'math', source: run.source };
    default:
      return { type: 'break' };
  }
}

const sameLook = (one: Inline, other: Inline): boolean => {
  if (one.type !== 'text' || other.type !== 'text') return false;
  const { text: _one, ...a } = one;
  const { text: _other, ...b } = other;
  return JSON.stringify(a) === JSON.stringify(b);
};

/** Inline content with adjacent text of one look joined and empty text dropped: what two equal lines both come to. */
export function canonInlines(runs: readonly Inline[]): Inline[] {
  const out: Inline[] = [];
  for (const run of runs) {
    const next = canonInline(run);
    if (next.type === 'text' && next.text === '') continue;
    const last = out[out.length - 1];
    if (last !== undefined && next.type === 'text' && sameLook(last, next) && last.type === 'text') {
      out[out.length - 1] = { ...last, text: last.text + next.text };
    } else out.push(next);
  }
  return out;
}

function isRows(content: Content): content is TableRow[] {
  const first = content[0] as { cells?: unknown } | undefined;
  return first !== undefined && Array.isArray(first.cells);
}

/** A block's content in its canonical form: inline content joined, or the rows of a table with their cells so. */
export function canonContent(content: Content | null | undefined): Content {
  if (content === null || content === undefined) return [];
  if (isRows(content)) {
    return content.map((row) => ({
      cells: row.cells.map((cell) => {
        const out: Record<string, unknown> = { content: canonInlines(cell.content) };
        if (typeof cell.colspan === 'number' && cell.colspan > 1) out.colspan = cell.colspan;
        if (typeof cell.rowspan === 'number' && cell.rowspan > 1) out.rowspan = cell.rowspan;
        if (typeof cell.color === 'number') out.color = cell.color;
        if (typeof cell.background === 'number') out.background = cell.background;
        return out as unknown as TableRow['cells'][number];
      }),
    }));
  }
  return canonInlines(content);
}

/** Whether a draft says what a block of the server says, whatever the form each came in. */
export function sameBlock(draft: Pick<Draft, 'type' | 'props' | 'content'>, row: Pick<DocumentBlock, 'type' | 'props' | 'content'>): boolean {
  if (draft.type !== row.type) return false;
  return (
    JSON.stringify(canonProps(draft.props)) === JSON.stringify(canonProps(row.props)) &&
    JSON.stringify(canonContent(draft.content)) === JSON.stringify(canonContent(row.content))
  );
}

/** The plain text of a block's content, for a title, a search and a quotation. */
export function plainText(content: Content | null | undefined): string {
  if (content === null || content === undefined) return '';
  if (isRows(content)) return content.map((row) => row.cells.map((cell) => plainText(cell.content)).join('\t')).join('\n');
  return (content)
    .map((run) => {
      switch (run.type) {
        case 'text':
          return run.text;
        case 'math':
          return run.source;
        case 'ref':
          return run.name ?? '';
        case 'mention':
          return run.user === undefined || run.user === null ? '' : `@${run.user.display_name}`;
        default:
          return '\n';
      }
    })
    .join('');
}

// ---------------------------------------------------------------- planning a batch

/** What changing one block into another came to: for the line under "Saving…" and for tests. */
export interface Plan {
  ops: DocumentOp[];
  /** Every block a batch of these touches. */
  touched: Set<string>;
}

/** The indices of one longest increasing run of `values`, in the order they stand. */
export function longestIncreasing(values: readonly number[]): number[] {
  const tails: number[] = [];
  const before: number[] = new Array<number>(values.length).fill(-1);
  for (let index = 0; index < values.length; index += 1) {
    const value = values[index] ?? 0;
    let low = 0;
    let high = tails.length;
    while (low < high) {
      const middle = (low + high) >> 1;
      if ((values[tails[middle] ?? 0] ?? 0) < value) low = middle + 1;
      else high = middle;
    }
    if (low > 0) before[index] = tails[low - 1] ?? -1;
    tails[low] = index;
  }
  const run: number[] = [];
  for (let at = tails[tails.length - 1] ?? -1; at >= 0; at = before[at] ?? -1) run.unshift(at);
  return run;
}

function isEmptyParagraph(draft: Draft): boolean {
  return draft.type === 'paragraph' && Array.isArray(draft.content) && draft.content.length === 0 && Object.keys(canonProps(draft.props)).length === 0;
}

export interface PlanOptions {
  /** Blocks that are known to say what the server's row says: they are not compared again. */
  unchanged?: ReadonlySet<string>;
  /** The stamp of the row a block's text was written on, when that is not the row the server holds now. */
  baseOf?: (id: string) => number | undefined;
}

/**
 * The operations that make the blocks of `base` the blocks of `drafts`.
 *
 * Updates come first, then the blocks put in place (inserted, or moved where the order or the parent
 * changed), then removals, so that a batch cut into pieces never updates a block it has itself moved
 * already, and a block that is taken out from under one that goes is moved before that one is removed.
 * Of the blocks that stay in one parent only those that are out of order are moved: the longest run
 * that is already in order stays where it is. A paragraph that is empty and last at the top level
 * and that the server does not have is not sent until something is written in it.
 */
export function planOps(base: Tree, drafts: readonly Draft[], options: PlanOptions = {}): Plan {
  const wanted = drafts.filter((draft, index) => !(index === drafts.length - 1 && draft.parentId === null && isEmptyParagraph(draft) && !base.rows.has(draft.id)));
  const want = new Map(wanted.map((draft) => [draft.id, draft]));
  /** Where each wanted block stands: a parent that is not among the wanted blocks is no parent. */
  const parentOf = new Map<string, string | null>();
  const wantedChildren = new Map<string | null, Draft[]>();
  for (const draft of wanted) {
    const parent = draft.parentId !== null && want.has(draft.parentId) ? draft.parentId : null;
    parentOf.set(draft.id, parent);
    const siblings = wantedChildren.get(parent);
    if (siblings === undefined) wantedChildren.set(parent, [draft]);
    else siblings.push(draft);
  }

  const updates: DocumentOp[] = [];
  const placing: DocumentOp[] = [];
  const removals: DocumentOp[] = [];
  const touched = new Set<string>();

  for (const draft of wanted) {
    const row = base.rows.get(draft.id);
    if (row === undefined || options.unchanged?.has(draft.id) === true || sameBlock(draft, row)) continue;
    const typeChanged = draft.type !== row.type;
    const props = canonProps(draft.props);
    const content = canonContent(draft.content);
    updates.push({
      op: 'update',
      id: draft.id,
      base_seq: options.baseOf?.(draft.id) ?? row.seq_updated,
      ...(typeChanged ? { type: draft.type } : {}),
      ...(typeChanged || JSON.stringify(props) !== JSON.stringify(canonProps(row.props)) ? { props: props as never } : {}),
      ...(JSON.stringify(content) !== JSON.stringify(canonContent(row.content)) ? { content } : {}),
    });
    touched.add(draft.id);
  }

  // Parents are visited before what is under them, so that a block is moved under one that stands where it should.
  const visit = (parent: string | null) => {
    const group = wantedChildren.get(parent) ?? [];
    const staying: { at: number; oldIndex: number }[] = [];
    // The blocks that are under this parent now and are to stay under it, in the order they stand.
    const oldList = (base.children.get(parent) ?? []).filter((id) => parentOf.get(id) === parent);
    const oldIndex = new Map(oldList.map((id, index) => [id, index]));
    group.forEach((draft, at) => {
      const old = oldIndex.get(draft.id);
      if (old !== undefined) staying.push({ at, oldIndex: old });
    });
    const stable = new Set(longestIncreasing(staying.map((entry) => entry.oldIndex)).map((index) => staying[index]?.at ?? -1));
    group.forEach((draft, at) => {
      const afterId = at === 0 ? null : (group[at - 1]?.id ?? null);
      const row = base.rows.get(draft.id);
      if (row === undefined) {
        placing.push({
          op: 'insert',
          block: { id: draft.id, type: draft.type, props: canonProps(draft.props), content: canonContent(draft.content) },
          parent_id: parent,
          after_id: afterId,
        });
        touched.add(draft.id);
      } else if (!stable.has(at)) {
        placing.push({ op: 'move', id: draft.id, parent_id: parent, after_id: afterId });
        touched.add(draft.id);
      }
      visit(draft.id);
    });
  };
  visit(null);

  for (const [id, row] of base.rows) {
    if (want.has(id)) continue;
    // What is under a block that goes goes with it.
    if (row.parent_id !== null && base.rows.has(row.parent_id) && !want.has(row.parent_id)) continue;
    removals.push({ op: 'remove', id });
    touched.add(id);
  }

  return { ops: [...updates, ...placing, ...removals], touched };
}

/** Cuts a plan into batches the server takes, keeping the order. */
export function inBatches(ops: readonly DocumentOp[], size: number = OPS_PER_BATCH): DocumentOp[][] {
  const batches: DocumentOp[][] = [];
  for (let at = 0; at < ops.length; at += size) batches.push(ops.slice(at, at + size));
  return batches;
}
