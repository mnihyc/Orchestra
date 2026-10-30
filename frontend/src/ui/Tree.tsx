/** Shared Tree component (collections); primitives are wrapped here.
 *
 * Things that lie inside one another, as rows that open and close. The keyboard works it as it
 * works a tree anywhere: up and down from row to row, right to open a row or step into it, left to
 * close it or step out of it, Home and End to its ends, a letter to the next row that begins with
 * it, Enter to go where the row leads.
 *
 * The tree is given its rows already laid out in order, each with its depth, so that a tree of
 * thousands is one flat list, and a row whose thing did not change is not drawn again. A row is not
 * even looked at again when nothing about it changed: the rows are drawn in runs of a few dozen, and a
 * run none of whose rows changed is passed over as one, so that a change to one row of two thousand
 * costs the drawing of one run and not the visiting of every row. A run that is far out of view is not
 * laid out, styled or painted by the browser at all (`content-visibility`), and takes the room its rows
 * would. Where a run begins is decided by its first row alone, so that a row opened or removed changes
 * the run it lies in and no run after it.
 *
 * A tree can be one of ticks: a row that carries `tick` has a box before its name, ticked, empty or half
 * ticked (`'mixed'`, a row whose rows below are some ticked and some not), and a press on the row, or the space
 * bar on it, asks `onTick` for the other state. The arrow keys still go from row to row and open and close
 * rows, so a list of choices that lies in places is chosen with the keyboard as it is walked.
 */
import { Check, ChevronRight, Minus } from 'lucide-react';
import {
  type CSSProperties,
  type KeyboardEvent,
  memo,
  type ReactElement,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';

import { isComposing } from '@/lib/shortcuts';
import { cx } from '@/ui/cx';

export interface TreeRow<Item> {
  id: string;
  /** How deep it lies, from 0. */
  depth: number;
  item: Item;
  /** Whether rows lie under it, so that it opens and closes. */
  expandable: boolean;
  expanded: boolean;
  /** The row it lies under, or `null` at the top. */
  parentId: string | null;
  /** Gives the row a box: ticked, empty, or half ticked. Absent, the row has none. */
  tick?: boolean | 'mixed';
  /** The box is shown but cannot be changed, for a thing that may not be chosen. */
  tickLocked?: boolean;
}

export interface TreeProps<Item> {
  /** A flat arrangement has no hierarchy gutter or reserved expander. */
  flat?: boolean;
  /** What the tree is of, for a screen reader. */
  label: string;
  /** The rows that are in view, in order from top to bottom. */
  rows: readonly TreeRow<Item>[];
  /** The row of the screen in view: marked, and scrolled to when it changes. */
  currentId?: string | null;
  onExpandedChange: (id: string, expanded: boolean) => void;
  /** Asked when a row with a box is pressed, or has the space bar pressed on it, for the state it is to be in. */
  onTick?: (row: TreeRow<Item>, on: boolean) => void;
  /** The name of a row, by which a typed letter finds it. */
  nameOf: (item: Item) => string;
  /**
   * What a row shows after its arrow. A link in it is where Enter goes. It must be the same function
   * from one drawing to the next for rows to be spared.
   */
  renderRow: (row: TreeRow<Item>, state: { current: boolean }) => ReactNode;
  /**
   * Puts something round the whole row, as its right-click menu does. Like `renderRow` it must be
   * the same function from one drawing to the next for rows to be spared.
   */
  wrapRow?: (row: TreeRow<Item>, element: ReactElement) => ReactElement;
  className?: string;
}

/** How long typed letters count as one word: the usual pause between two keys of a name, and no more. */
const TYPE_AHEAD_MS = 600;

/** How many levels are indented by a whole step: six is as deep as a person reads a tree by its indentation alone. */
const FULL_STEP_LEVELS = 6;

/** About how many rows make a run: few enough that a run is drawn again within a frame, many enough that two thousand rows are some fifty runs to look at. */
const RUN_ROWS = 40;
/** The most rows in a run, so that a stretch of rows none of which begins one is still cut. */
const RUN_MAX_ROWS = 120;

/** Whether a row begins a run: about one row in `RUN_ROWS`, by a hash of its id (FNV-1a), which is the same wherever the row stands. */
function beginsRun(id: string): boolean {
  let hash = 0x811c9dc5;
  for (let at = 0; at < id.length; at += 1) {
    hash ^= id.charCodeAt(at);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0) % RUN_ROWS === 0;
}

interface RowProps<Item> {
  flat: boolean;
  row: TreeRow<Item>;
  current: boolean;
  tabbable: boolean;
  position: number;
  setSize: number;
  renderRow: TreeProps<Item>['renderRow'];
  wrapRow: TreeProps<Item>['wrapRow'];
  onToggle: (id: string, expanded: boolean) => void;
  onTick: TreeProps<Item>['onTick'];
  onFocusRow: (id: string) => void;
}

/** The box of a row with ticks, drawn as the shared checkbox is: the row itself is what a screen reader and the keyboard reach. */
function TickBox({ state, locked }: { state: boolean | 'mixed'; locked: boolean }) {
  return (
    <span
      aria-hidden
      data-state={state === 'mixed' ? 'indeterminate' : state ? 'checked' : 'unchecked'}
      className={cx(
        'inline-flex size-4 shrink-0 items-center justify-center rounded-sm border transition-colors',
        'border-border-strong bg-surface group-hover/row:border-text-subtle',
        'data-[state=checked]:border-accent-solid data-[state=checked]:bg-accent-solid data-[state=checked]:text-text-on-accent',
        'data-[state=indeterminate]:border-accent-solid data-[state=indeterminate]:bg-accent-solid data-[state=indeterminate]:text-text-on-accent',
        locked && 'border-border bg-bg-muted text-text-disabled group-hover/row:border-border data-[state=checked]:border-border data-[state=checked]:bg-bg-muted data-[state=checked]:text-text-disabled',
      )}
    >
      {state === 'mixed' ? <Minus size={12} strokeWidth={3} /> : state ? <Check size={12} strokeWidth={3} /> : null}
    </span>
  );
}

function RowInner<Item>({
  flat,
  row,
  current,
  tabbable,
  position,
  setSize,
  renderRow,
  wrapRow,
  onToggle,
  onTick,
  onFocusRow,
}: RowProps<Item>) {
  // Computed geometry: a step of the spacing scale for each level, and half a step for each level beyond the sixth, so that a
  // tree that goes very deep (folders in folders) still leaves its names some room.
  const deeper = Math.max(0, row.depth - FULL_STEP_LEVELS);
  const indent: CSSProperties = {
    paddingInlineStart: flat ? 'var(--spacing-2)' : `calc(var(--spacing-4) * ${row.depth - deeper} + var(--spacing-2) * ${deeper} + var(--spacing-1))`,
  };
  const element = (
    <div
      role="treeitem"
      data-tree-row={row.id}
      aria-level={row.depth + 1}
      aria-posinset={position}
      aria-setsize={setSize}
      aria-expanded={row.expandable ? row.expanded : undefined}
      aria-selected={current}
      aria-checked={row.tick === undefined ? undefined : row.tick === 'mixed' ? 'mixed' : row.tick}
      aria-current={current ? 'page' : undefined}
      tabIndex={tabbable ? 0 : -1}
      style={indent}
      onClick={
        row.tick === undefined || onTick === undefined
          ? undefined
          : (event) => {
              // A press on a link or a button in the row is theirs.
              if (row.tickLocked === true || (event.target as HTMLElement).closest('a, button') !== null) return;
              onTick(row, row.tick !== true);
            }
      }
      className={cx(
        'group/row focus-inset relative mx-1 flex h-(--row-height) cursor-default items-center gap-1 rounded-md pr-2 text-base select-none',
        current ? 'bg-accent-subtle text-text' : 'text-text-muted hover:bg-bg-muted hover:text-text',
      )}
      onFocus={(event) => {
        if (event.target === event.currentTarget) onFocusRow(row.id);
      }}
    >
      {!flat && <span
        aria-hidden
        className={cx(
          'flex size-4 shrink-0 items-center justify-center rounded-sm text-text-subtle',
          row.expandable ? 'hover:bg-border hover:text-text' : 'invisible',
        )}
        onClick={(event) => {
          event.preventDefault();
          event.stopPropagation();
          onToggle(row.id, !row.expanded);
        }}
      >
        <ChevronRight className={cx('transition-transform', row.expanded && 'rotate-90')} />
      </span>}
      {row.tick !== undefined && <TickBox state={row.tick} locked={row.tickLocked === true} />}
      {renderRow(row, { current })}
    </div>
  );
  return wrapRow === undefined ? element : wrapRow(row, element);
}

const Row = memo(RowInner, (before, after) => {
  return (
    before.flat === after.flat &&
    before.current === after.current &&
    before.tabbable === after.tabbable &&
    before.position === after.position &&
    before.setSize === after.setSize &&
    before.renderRow === after.renderRow &&
    before.wrapRow === after.wrapRow &&
    before.onToggle === after.onToggle &&
    before.onTick === after.onTick &&
    before.onFocusRow === after.onFocusRow &&
    before.row.id === after.row.id &&
    before.row.tick === after.row.tick &&
    before.row.tickLocked === after.row.tickLocked &&
    before.row.item === after.row.item &&
    before.row.depth === after.row.depth &&
    before.row.expandable === after.row.expandable &&
    before.row.expanded === after.row.expanded
  );
}) as typeof RowInner;

/** What a row is drawn from, kept as one thing so that a row, and a run of rows, can be told apart from what they were. */
interface RowView<Item> {
  row: TreeRow<Item>;
  current: boolean;
  tabbable: boolean;
  position: number;
  setSize: number;
  /** Whether a run of rows begins with it; the same wherever it stands. */
  begins: boolean;
}

/** Whether a view still says what a row, with where it stands, says. */
function stillSays<Item>(view: RowView<Item>, row: TreeRow<Item>, current: boolean, tabbable: boolean, position: number, setSize: number): boolean {
  if (view.current !== current || view.tabbable !== tabbable || view.position !== position || view.setSize !== setSize) return false;
  const held = view.row;
  return (
    held === row ||
    (held.id === row.id &&
      held.item === row.item &&
      held.depth === row.depth &&
      held.expandable === row.expandable &&
      held.expanded === row.expanded &&
      held.tick === row.tick &&
      held.tickLocked === row.tickLocked &&
      held.parentId === row.parentId)
  );
}

interface RunProps<Item> {
  flat: boolean;
  views: readonly RowView<Item>[];
  renderRow: TreeProps<Item>['renderRow'];
  wrapRow: TreeProps<Item>['wrapRow'];
  onToggle: (id: string, expanded: boolean) => void;
  onTick: TreeProps<Item>['onTick'];
  onFocusRow: (id: string) => void;
}

/**
 * A run of rows, drawn again only when one of them is another view than it was. It stands in an element that has no role of its own
 * and that the browser may leave out of its layout, styling and painting while it is far out of view, and until then it keeps the
 * room its rows take (they all have the height of a row).
 */
function RunInner<Item>({ flat, views, renderRow, wrapRow, onToggle, onTick, onFocusRow }: RunProps<Item>) {
  return (
    <div
      role="none"
      className="shrink-0 [content-visibility:auto]"
      style={{ containIntrinsicBlockSize: `calc(var(--row-height) * ${views.length})` }}
    >
      {views.map((view) => (
        <Row
          flat={flat}
          key={view.row.id}
          row={view.row}
          current={view.current}
          tabbable={view.tabbable}
          position={view.position}
          setSize={view.setSize}
          renderRow={renderRow}
          wrapRow={wrapRow}
          onToggle={onToggle}
          onTick={onTick}
          onFocusRow={onFocusRow}
        />
      ))}
    </div>
  );
}

const Run = memo(RunInner, (before, after) => {
  if (
    before.flat !== after.flat ||
    before.renderRow !== after.renderRow ||
    before.wrapRow !== after.wrapRow ||
    before.onToggle !== after.onToggle ||
    before.onTick !== after.onTick ||
    before.onFocusRow !== after.onFocusRow ||
    before.views.length !== after.views.length
  ) {
    return false;
  }
  return before.views.every((view, at) => view === after.views[at]);
}) as typeof RunInner;

export function Tree<Item>({
  flat = false,
  label,
  rows,
  currentId = null,
  onExpandedChange,
  onTick,
  nameOf,
  renderRow,
  wrapRow,
  className,
}: TreeProps<Item>) {
  const container = useRef<HTMLDivElement>(null);
  const [focusId, setFocusId] = useState<string | null>(null);
  const typed = useRef({ text: '', at: 0 });

  // Where each row stands among the rows under the same one, for a screen reader's "3 of 7".
  const places = useMemo(() => {
    const sizes = new Map<string | null, number>();
    const position = new Map<string, number>();
    for (const row of rows) {
      const count = (sizes.get(row.parentId) ?? 0) + 1;
      sizes.set(row.parentId, count);
      position.set(row.id, count);
    }
    return { sizes, position };
  }, [rows]);

  // One row takes the Tab key: the one last focused if it is still in view, else the current
  // screen's, else the first.
  const tabId = useMemo(() => {
    if (focusId !== null && rows.some((row) => row.id === focusId)) return focusId;
    if (currentId !== null && rows.some((row) => row.id === currentId)) return currentId;
    return rows[0]?.id ?? null;
  }, [focusId, currentId, rows]);

  // The latest of what the caller gave to open and close a row, behind one function that stays the same, so that a caller
  // whose function changes with every drawing does not make every run be drawn again.
  const toggle = useRef(onExpandedChange);
  useEffect(() => {
    toggle.current = onExpandedChange;
  }, [onExpandedChange]);
  const onToggle = useCallback((id: string, expanded: boolean) => toggle.current(id, expanded), []);
  // The same for what a press on a box asks: one function that stays the same while the tree has ticks at all.
  const tick = useRef(onTick);
  useEffect(() => {
    tick.current = onTick;
  }, [onTick]);
  const ticking = onTick !== undefined;
  const onBox = useCallback((row: TreeRow<Item>, on: boolean) => tick.current?.(row, on), []);

  // Each row as what it is drawn from, the same object as before where nothing about it changed, in runs.
  const [kept] = useState(() => new Map<string, RowView<Item>>());
  const runs = useMemo(() => {
    const views: RowView<Item>[] = [];
    for (const row of rows) {
      const current = row.id === currentId;
      const tabbable = row.id === tabId;
      const position = places.position.get(row.id) ?? 1;
      const setSize = places.sizes.get(row.parentId) ?? 1;
      const held = kept.get(row.id);
      if (held !== undefined && stillSays(held, row, current, tabbable, position, setSize)) {
        views.push(held);
        continue;
      }
      const made: RowView<Item> = { row, current, tabbable, position, setSize, begins: held?.begins ?? beginsRun(row.id) };
      kept.set(row.id, made);
      views.push(made);
    }
    // What was kept of rows that are no longer in view is let go.
    if (kept.size > views.length) {
      const inView = new Set(views.map((view) => view.row.id));
      for (const id of kept.keys()) if (!inView.has(id)) kept.delete(id);
    }
    const made: RowView<Item>[][] = [];
    let run: RowView<Item>[] = [];
    for (const view of views) {
      if (run.length > 0 && (run.length >= RUN_MAX_ROWS || view.begins)) {
        made.push(run);
        run = [];
      }
      run.push(view);
    }
    if (run.length > 0) made.push(run);
    return made;
  }, [rows, currentId, tabId, places, kept]);

  const elementOf = useCallback(
    (id: string) => container.current?.querySelector<HTMLElement>(`[data-tree-row="${CSS.escape(id)}"]`) ?? null,
    [],
  );

  const focusRow = useCallback(
    (id: string | undefined) => {
      if (id === undefined) return;
      setFocusId(id);
      elementOf(id)?.focus();
    },
    [elementOf],
  );

  // The row of the screen in view is brought into view when the screen changes.
  useEffect(() => {
    if (currentId !== null) elementOf(currentId)?.scrollIntoView({ block: 'nearest' });
  }, [currentId, elementOf]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (isComposing(event.nativeEvent) || event.altKey) return;
    const target = event.target as HTMLElement;
    // Keys pressed inside something a row holds (a button, a field) are that thing's.
    if (target.getAttribute('role') !== 'treeitem') return;
    // Mod+Enter opens where the row leads in a new browser tab, as a middle click on its link does.
    if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
      const link = target.querySelector<HTMLAnchorElement>('a[href]');
      if (link !== null) window.open(link.href, '_blank', 'noopener');
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (event.ctrlKey || event.metaKey) return;
    const index = rows.findIndex((row) => row.id === target.dataset.treeRow);
    const row = rows[index];
    if (row === undefined) return;

    const done = () => {
      event.preventDefault();
      event.stopPropagation();
    };

    switch (event.key) {
      case 'ArrowDown':
        focusRow(rows[index + 1]?.id);
        done();
        return;
      case 'ArrowUp':
        focusRow(rows[index - 1]?.id);
        done();
        return;
      case 'Home':
        focusRow(rows[0]?.id);
        done();
        return;
      case 'End':
        focusRow(rows.at(-1)?.id);
        done();
        return;
      case 'ArrowRight':
        if (row.expandable && !row.expanded) onExpandedChange(row.id, true);
        else if (row.expandable && rows[index + 1]?.parentId === row.id) focusRow(rows[index + 1]?.id);
        done();
        return;
      case 'ArrowLeft':
        if (row.expandable && row.expanded) onExpandedChange(row.id, false);
        else if (row.parentId !== null) focusRow(row.parentId);
        done();
        return;
      case 'Enter':
        target.querySelector<HTMLElement>('a[href]')?.click();
        done();
        return;
      case ' ':
        if (row.tick !== undefined && ticking) {
          if (row.tickLocked !== true) onBox(row, row.tick !== true);
        } else if (row.expandable) onExpandedChange(row.id, !row.expanded);
        done();
        return;
      default:
    }

    if (event.key.length === 1 && event.key !== ' ' && !event.shiftKey) {
      const now = Date.now();
      const text = (now - typed.current.at > TYPE_AHEAD_MS ? '' : typed.current.text) + event.key.toLocaleLowerCase();
      typed.current = { text, at: now };
      // A single letter goes on to the next match; more letters refine where the search stands.
      const from = text.length === 1 ? index + 1 : index;
      for (let step = 0; step < rows.length; step += 1) {
        const candidate = rows[(from + step) % rows.length];
        if (candidate !== undefined && nameOf(candidate.item).toLocaleLowerCase().startsWith(text)) {
          focusRow(candidate.id);
          done();
          return;
        }
      }
    }
  };

  return (
    <div ref={container} role="tree" aria-label={label} className={cx('flex flex-col', className)} onKeyDown={onKeyDown}>
      {runs.map((views) => (
        <Run
          flat={flat}
          key={views[0]?.row.id}
          views={views}
          renderRow={renderRow}
          wrapRow={wrapRow}
          onToggle={onToggle}
          onTick={ticking ? onBox : undefined}
          onFocusRow={setFocusId}
        />
      ))}
    </div>
  );
}
