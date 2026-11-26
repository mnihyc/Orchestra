/** Picking rows of a list to act on them together, as file managers and VS Code do.
 *
 * A box at the start of each row ticks it; `Shift` with a box or a click on the row ticks everything between it and
 * the last row touched; `Ctrl` (`⌘` on a Mac) with a click on the row ticks or unticks that row alone; the box at the
 * head of the list ticks every loaded row. A row that leaves the list leaves the selection. The ticked rows' acts
 * stand in one bar above the list, with the count and Clear.
 */
import type { ReactNode } from 'react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';

/** Ticks (`on`) or unticks one id, or with `range` everything from `anchor` to it among the loaded ids; ticks elsewhere stay. */
export function selectRange(selected: ReadonlySet<string>, loaded: readonly string[], anchor: string | null, id: string, on: boolean, range: boolean): Set<string> {
  const next = new Set(selected);
  const first = anchor === null ? -1 : loaded.indexOf(anchor);
  const last = loaded.indexOf(id);
  const ids = range && first >= 0 && last >= 0 ? loaded.slice(Math.min(first, last), Math.max(first, last) + 1) : [id];
  for (const one of ids) {
    if (on) next.add(one);
    else next.delete(one);
  }
  return next;
}

/** The keys of a click that picks rather than opens. */
export interface PickingClick {
  shiftKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
}

export interface RowSelection<Row> {
  /** The ticked rows, in the list's order. */
  selected: Row[];
  isTicked: (row: Row) => boolean;
  /** Ticks or unticks a row; with `range`, everything between the last row touched and this one. */
  toggle: (row: Row, on: boolean, range?: boolean) => void;
  /** What the box at the head of the list shows. */
  all: boolean | 'indeterminate';
  toggleAll: (on: boolean) => void;
  clear: () => void;
  /** A click on a row with `Shift`, `Ctrl` or `⌘` picks; says whether it did, so that a plain click still opens. */
  click: (row: Row, event: PickingClick) => boolean;
}

export function useRowSelection<Row>(rows: readonly Row[], keyOf: (row: Row) => string): RowSelection<Row> {
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set());
  const [anchor, setAnchor] = useState<string | null>(null);
  const selected = rows.filter((row) => ticked.has(keyOf(row)));
  const toggle = (row: Row, on: boolean, range = false) => {
    const key = keyOf(row);
    setTicked((before) => selectRange(before, rows.map(keyOf), anchor, key, on, range));
    setAnchor(key);
  };
  return {
    selected,
    isTicked: (row) => ticked.has(keyOf(row)),
    toggle,
    all: selected.length === rows.length && rows.length > 0 ? true : selected.length === 0 ? false : 'indeterminate',
    toggleAll: (on) => setTicked(on ? new Set(rows.map(keyOf)) : new Set()),
    clear: () => {
      setTicked(new Set());
      setAnchor(null);
    },
    click: (row, event) => {
      if (event.shiftKey) {
        toggle(row, true, true);
        globalThis.getSelection?.()?.removeAllRanges();
        return true;
      }
      if (event.ctrlKey || event.metaKey) {
        toggle(row, !ticked.has(keyOf(row)));
        return true;
      }
      return false;
    },
  };
}

/** Whether a click landed on something of the row that acts by itself (its link, a button, a box, a field). */
export function onOwnControl(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest('a,button,input,textarea,select,label,[role="checkbox"],[role="menuitem"],[contenteditable="true"]') !== null;
}

const selectionFrame = 'inline-flex min-h-(--control-sm) items-center [&>div]:self-center [&_.choice-toggle]:mt-0';

/** The box that ticks one row, named by the row; `Shift` with it ticks the range from the last row touched. */
export function RowBox({ name, checked, onChange }: { name: string; checked: boolean; onChange: (on: boolean, range: boolean) => void }) {
  const { t } = useTranslation();
  const range = useRef(false);
  return (
    <div className={selectionFrame} onClickCapture={(event) => { range.current = event.shiftKey; }}>
      <Checkbox presentation="compact" label={<span className="sr-only">{t('resources.bulk.select', { name })}</span>} checked={checked} onCheckedChange={(on) => { onChange(on, range.current); range.current = false; }} />
    </div>
  );
}

/** The box at the head of the list, which ticks every loaded row. */
export function AllBox({ checked, onChange }: { checked: boolean | 'indeterminate'; onChange: (on: boolean) => void }) {
  const { t } = useTranslation();
  return <div className={selectionFrame}><Checkbox presentation="compact" label={<span className="sr-only">{t('resources.bulk.selectAll')}</span>} checked={checked} onCheckedChange={onChange} /></div>;
}

/**
 * The bar of what the ticked rows share: their count, Select all where a list has no box at its head (`total` rows
 * loaded, `onSelectAll`), their acts, and Clear.
 */
export function SelectedBar({ count, total, onSelectAll, label, busy = false, onClear, children }: {
  count: number; total?: number; onSelectAll?: () => void; label?: string; busy?: boolean; onClear: () => void; children?: ReactNode;
}) {
  const { t } = useTranslation();
  return (
    // It stays in view while the list scrolls under it, so that rows far down can be picked and acted on.
    <div role="toolbar" aria-label={label ?? t('resources.bulk.label')} className="sticky top-0 z-(--layer-sticky) flex flex-wrap items-center gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2">
      <span className="text-sm font-medium text-text" aria-live="polite">{t('resources.bulk.count', { count })}</span>
      {onSelectAll !== undefined && total !== undefined && count < total && (
        <Button size="sm" variant="ghost" disabled={busy} onClick={onSelectAll}>
          {t('resources.bulk.selectAll')}
        </Button>
      )}
      {children}
      <Button size="sm" variant="ghost" disabled={busy} onClick={onClear}>
        {t('resources.bulk.clear')}
      </Button>
    </div>
  );
}
