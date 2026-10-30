/** What a document loses when it is written as Markdown, counted from its blocks before the file is made, in the kinds the server tells it by afterwards.
 *
 * Markdown has no form for a callout, a toggle, columns, colours, alignment and some more, and writes each as the nearest thing it has. The
 * server says what it could not keep once the file is written; this says the same of the same blocks before, so that a person who chooses
 * Markdown knows what they are choosing. The kinds, their order and what each counts follow the server's: a block is counted once for
 * a kind, whatever number of times it loses that kind of thing, and the kinds that are no number (colours, alignment, underlines, the widths
 * of a table) are said once.
 */
import type { DocumentBlock, Inline, TableRow } from '@/generated/api-types';

/** The kinds of loss, in the order the server tells them. */
export const LOSS_KINDS = [
  'callouts',
  'toggles',
  'foldingHeadings',
  'columns',
  'spans',
  'underlines',
  'colours',
  'alignment',
  'nesting',
  'headerlessTables',
  'tableWidths',
  'mediaAddresses',
] as const;

export type LossKind = (typeof LOSS_KINDS)[number];

/** How many blocks lost something of one kind. */
export interface Loss {
  kind: LossKind;
  count: number;
}

/** The kinds whose sentence has no number in it: it says that the thing is left out, not how often. */
export const WITHOUT_COUNT: ReadonlySet<LossKind> = new Set<LossKind>(['underlines', 'colours', 'alignment', 'tableWidths']);

/** The blocks that are items of a list: a block under one of them stays under it. */
const LIST_ITEMS: ReadonlySet<string> = new Set(['bullet', 'numbered', 'todo', 'toggle']);
/** The blocks that hold others without losing them: a quote and a callout hold what is under them. */
const HOLDERS: ReadonlySet<string> = new Set(['quote', 'callout']);
/** The blocks that name a file or an address, and are written as a link to it. */
const MEDIA: ReadonlySet<string> = new Set(['image', 'video', 'audio', 'file']);

/** Whether a prop, a colour or a width is there: a value that is `null` is the same as one that was left out. */
const given = (value: unknown): boolean => value !== null && value !== undefined;

/** Whether any run of text carries what Markdown leaves out. */
function inRuns(runs: readonly Inline[], found: Set<LossKind>): void {
  for (const run of runs) {
    if (run.type !== 'text') continue;
    if (run.marks?.includes('underline') === true) found.add('underlines');
    if (given(run.color) || given(run.background)) found.add('colours');
  }
}

/** What a table loses: its header row, the widths of its columns, cells that span, and the colours of its cells and of the text in them. */
function inTable(props: Record<string, unknown>, rows: readonly TableRow[], found: Set<LossKind>): void {
  // A table with no row is written as nothing, and loses nothing.
  if (rows.length === 0) return;
  if (props.header_row !== true) found.add('headerlessTables');
  if (given(props.widths)) found.add('tableWidths');
  for (const row of rows) {
    for (const cell of row.cells) {
      if ((cell.colspan ?? 1) > 1 || (cell.rowspan ?? 1) > 1) found.add('spans');
      if (given(cell.color) || given(cell.background)) found.add('colours');
      inRuns(cell.content, found);
    }
  }
}

/** What Markdown cannot keep of a document's blocks, each kind with the number of blocks that lose it; nothing where it keeps all. */
export function markdownLosses(blocks: readonly DocumentBlock[]): Loss[] {
  const live = blocks.filter((block) => !block.removed);
  const withChildren = new Set(live.flatMap((block) => (block.parent_id === null ? [] : [block.parent_id])));
  const counts = new Map<LossKind, number>();
  for (const block of live) {
    const found = new Set<LossKind>();
    const props = block.props as Record<string, unknown>;
    if (block.type === 'columns') {
      // Columns are written one block after another, and what lies in them is told of by its own blocks.
      found.add('columns');
    } else if (block.type !== 'column') {
      if (given(props.color) || given(props.background)) found.add('colours');
      if (given(props.align) && props.align !== 'left') found.add('alignment');
      if (block.type === 'callout') found.add('callouts');
      if (block.type === 'toggle') found.add('toggles');
      if (block.type === 'heading' && props.toggle === true) found.add('foldingHeadings');
      if (MEDIA.has(block.type) && !given(props.file_id) && given(props.url)) found.add('mediaAddresses');
      if (block.type === 'table') inTable(props, block.content as TableRow[], found);
      else inRuns(block.content as Inline[], found);
      if (withChildren.has(block.id) && !LIST_ITEMS.has(block.type) && !HOLDERS.has(block.type)) found.add('nesting');
    }
    for (const kind of found) counts.set(kind, (counts.get(kind) ?? 0) + 1);
  }
  return LOSS_KINDS.flatMap((kind) => {
    const count = counts.get(kind);
    return count === undefined ? [] : [{ kind, count }];
  });
}
