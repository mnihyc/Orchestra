/** Shared Table component (collections); primitives are wrapped here.
 *
 * Rows of the same things with the same facts about each, under names for the columns. The first
 * column is what the row is, and holds the link that opens it. A column that cannot fit is left out
 * as the room it lies in narrows, and what it said moves into the first column's own lines: the
 * screen draws those for a narrow place, so a table becomes a list of cards without a second layout.
 * A table whose rows have acts gives each row its menu, which opens under the right mouse button and
 * with the menu key and `Shift+F10` for whatever of the row has the keyboard.
 */
import { defaultRangeExtractor, useVirtualizer } from '@tanstack/react-virtual';
import { Fragment, type ReactNode, useCallback, useLayoutEffect, useState } from 'react';

import { ContextEntries, ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import type { MenuEntry } from '@/ui/Menu';
import { ScrollRegion } from '@/ui/ScrollRegion';

/** How wide the room round the table is, in pixels, when each step of `from` begins to show a column. */
const ROOM = { sm: 480, md: 640, lg: 800, xl: 1000 } as const;

export type Room = keyof typeof ROOM;

export interface Column<Row> {
  id: string;
  header: ReactNode;
  /** Keeps the header for screen readers only: the column of a row's menu. */
  hideHeader?: boolean;
  /**
   * What the column says of a row. `shown` holds the ids of the columns drawn at this width, so that a
   * cell can say what a column that was left out would have said.
   */
  cell: (row: Row, shown: ReadonlySet<string>) => ReactNode;
  /** The narrowest room the column is shown in; always shown when absent. */
  from?: Room;
  /** The one row-identity column that takes spare room; defaults to `name`, then the first column.
   * Other columns keep their content width. Set false on a leading selection column with another identity id.
   */
  grow?: boolean;
  align?: 'start' | 'end';
  /** Classes of the header cell and of every cell, for a width. */
  className?: string;
}

export interface TableProps<Row> {
  /** What the table is of, for a screen reader. */
  label: string;
  columns: readonly Column<Row>[];
  rows: readonly Row[];
  rowKey: (row: Row) => string;
  /** Named resource lists bound identities and disclose full values on hover/focus; wide data tables retain intrinsic sizing. */
  boundedIdentity?: boolean;
  /** Keep the table bounded and draw only visible rows when a long, paged list has been loaded. */
  virtual?: boolean;
  /** Classes of a row, to say that it is archived or marked. */
  rowClassName?: (row: Row) => string | undefined;
  /**
   * The menu of a row: the same entries as the "⋯" at its end. They are made only for the row whose
   * menu is opened, so that a long table makes nothing for the rows nobody asked about.
   */
  rowMenu?: {
    label: (row: Row) => string;
    /** The acts of a row that have a key of their own, by the key (`F2`): pressed while something of the row has the keyboard, and not while it is being typed in. */
    keys?: (row: Row) => Readonly<Record<string, () => void>>;
  } & ({ entries: (row: Row) => readonly MenuEntry[]; content?: never } | { entries?: never; content: (row: Row) => ReactNode });
  className?: string;
}

/** The entries of one row, made as its menu opens. */
function RowEntries<Row>({ row, entries }: { row: Row; entries: (row: Row) => readonly MenuEntry[] }) {
  return <ContextEntries entries={entries(row)} />;
}

/** The width of the room an element lies in, kept as it changes. */
function useRoom(): [(element: HTMLDivElement | null) => void, number, HTMLDivElement | null] {
  const [element, setElement] = useState<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(Number.POSITIVE_INFINITY);
  const attach = useCallback((next: HTMLDivElement | null) => {
    setElement(next);
    if (next !== null) setWidth(next.clientWidth);
  }, []);
  useLayoutEffect(() => {
    if (element === null) return;
    const watch = new ResizeObserver(() => setWidth(element.clientWidth));
    watch.observe(element);
    return () => watch.disconnect();
  }, [element]);
  return [attach, width, element];
}

export function Table<Row>({ label, columns, rows, rowKey, rowClassName, rowMenu, className, boundedIdentity = false, virtual = false }: TableProps<Row>) {
  const [ref, room, scroller] = useRoom();
  const shown = columns.filter((column) => column.from === undefined || room >= ROOM[column.from]);
  const shownIds = new Set(shown.map((column) => column.id));
  const growing = shown.find((column) => column.grow === true)
    ?? shown.find((column) => column.id === 'name' && column.grow !== false)
    ?? shown.find((column) => column.grow !== false);
  // Bounded resource identities reserve twelve character widths and use the remaining
  // room. Engineering tables keep their intrinsic widths and the native named scroller.
  const sizing = (column: Column<Row>) => {
    if (column !== growing) return 'w-px whitespace-nowrap';
    return boundedIdentity ? 'w-auto min-w-[12ch] whitespace-normal' : 'w-auto whitespace-normal';
  };
  const line = (row: Row, index?: number, measure?: (element: HTMLTableRowElement | null) => void) => {
    const content = (
      <tr
        ref={measure}
        data-index={index}
        aria-rowindex={index === undefined ? undefined : index + 2}
        onKeyDown={
          rowMenu?.keys === undefined
            ? undefined
            : (event) => {
                const target = event.target;
                if (target instanceof HTMLElement && (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName))) return;
                const act = rowMenu.keys?.(row)[event.key];
                if (act === undefined || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
                event.preventDefault();
                act();
              }
        }
        className={cx(
          'group h-(--table-row-height) border-b border-border last:border-b-0 hover:bg-bg-subtle focus-within:bg-bg-subtle',
          rowClassName?.(row),
        )}
      >
        {shown.map((column) => (
          <td
            key={column.id}
            data-table-column={column.id}
            data-table-grow={column === growing ? true : undefined}
            className={cx('px-3 py-1 align-middle', sizing(column), column.align === 'end' && 'text-right', column.className)}
          >
            {boundedIdentity && column === growing ? <div className="w-0 min-w-full">{column.cell(row, shownIds)}</div> : column.cell(row, shownIds)}
          </td>
        ))}
      </tr>
    );
    return rowMenu === undefined ? content : (
      <ContextMenu label={rowMenu.label(row)} content={rowMenu.content === undefined ? <RowEntries row={row} entries={rowMenu.entries} /> : rowMenu.content(row)}>
        {content}
      </ContextMenu>
    );
  };
  // One page of 50 remains a native table; additional pages need a separate scroll window.
  const windowed = virtual && rows.length > 50;
  return (
    <ScrollRegion ref={ref} kind="table" label={label} className={cx('rounded-lg border border-border bg-surface', virtual && 'max-h-96 overflow-y-auto', className)}>
      <table aria-label={label} aria-rowcount={windowed ? rows.length + 1 : undefined} className="w-full table-auto border-collapse text-left text-base">
        <thead className="sticky top-0 z-(--layer-sticky)">
          <tr aria-rowindex={windowed ? 1 : undefined} className="border-b border-border bg-bg-subtle text-sm font-medium text-text-muted">
            {shown.map((column) => (
              <th
                key={column.id}
                scope="col"
                data-table-column={column.id}
                data-table-grow={column === growing ? true : undefined}
                className={cx('h-(--table-row-height) bg-bg-subtle px-3 font-medium', sizing(column), column.align === 'end' && 'text-right', column.className)}
              >
                <span className={cx(column.hideHeader === true && 'sr-only')}>{column.header}</span>
              </th>
            ))}
          </tr>
        </thead>
        {windowed ? <VirtualBody rows={rows} rowKey={rowKey} scroller={scroller} columns={shown.length} line={line} />
          : <tbody>{rows.map((row) => <Fragment key={rowKey(row)}>{line(row)}</Fragment>)}</tbody>}
      </table>
    </ScrollRegion>
  );
}

/** A virtual table retains native rows and measures them as their contents change. */
function VirtualBody<Row>({ rows, rowKey, scroller, columns, line }: {
  rows: readonly Row[];
  rowKey: (row: Row) => string;
  scroller: HTMLDivElement | null;
  columns: number;
  line: (row: Row, index: number, measure: (element: HTMLTableRowElement | null) => void) => ReactNode;
}) {
  const [focused, setFocused] = useState<number | null>(null);
  // The virtualizer owns the measured positions of the rows.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scroller,
    getItemKey: (index) => {
      const row = rows[index];
      return row === undefined ? index : rowKey(row);
    },
    // Two lines are a useful first estimate; each real row is measured when it appears.
    estimateSize: () => 72,
    overscan: 8,
    initialRect: { width: 0, height: 384 },
    // Scrolling must not unmount a row while a keyboard user is acting in it.
    rangeExtractor: (range) => {
      const visible = defaultRangeExtractor(range);
      return focused === null || focused < 0 || focused >= rows.length || visible.includes(focused)
        ? visible : [...visible, focused].sort((one, other) => one - other);
    },
  });
  const items = virtualizer.getVirtualItems();
  const drawn: ReactNode[] = [];
  let end = 0;
  const gap = (height: number, key: string) => height > 0 && <tr key={key} aria-hidden="true"><td colSpan={columns} style={{ height, padding: 0, border: 0 }} /></tr>;
  for (const item of items) {
    const row = rows[item.index];
    if (row === undefined) continue;
    drawn.push(gap(item.start - end, `before:${item.index}`));
    drawn.push(<Fragment key={item.key}>{line(row, item.index, virtualizer.measureElement)}</Fragment>);
    end = item.end;
  }
  drawn.push(gap(virtualizer.getTotalSize() - end, 'after'));
  return <tbody onFocusCapture={(event) => {
    const row = event.target instanceof Element ? event.target.closest<HTMLTableRowElement>('tr[data-index]') : null;
    if (row !== null) setFocused(Number(row.dataset.index));
  }} onBlurCapture={(event) => {
    if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setFocused(null);
  }}>
    {drawn}
  </tbody>;
}
